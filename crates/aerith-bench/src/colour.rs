// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The colour descriptions mpv set on its surface, read from mpv's own Wayland log, the same way
//! for mpv alone and for mpv inside Aerith's window
//! ([ADR-0024](https://github.com/HoneySlatt/Aerith/blob/main/docs/adr/0024-the-relay-passes-mpvs-colour-description.md)).
//!
//! With `WAYLAND_DEBUG=client`, libwayland prints each request mpv sends and each event it
//! receives. mpv builds a description with a parametric creator (`set_tf_named`,
//! `set_primaries_named`, `set_luminances`, the mastering display, MaxCLL, MaxFALL), `create`s
//! it, waits for the compositor's `ready` or `failed`, then sets it on its surface's
//! colour-management object. It also asks the compositor which description the surface prefers,
//! and reads it (`get_information`). Through Aerith's relay, the answers are the real
//! compositor's, passed back.
//!
//! The figures keep each request's arguments as libwayland prints them (enumerations as numbers,
//! luminances in the protocol's units), so that mpv alone's and Aerith's compare as they are:
//!
//! - **set**: each description mpv set on its surface, in order, with the compositor's answer;
//! - **unset**: how many times mpv removed its description;
//! - **failed**: how many descriptions the compositor refused;
//! - **preferred**: the last description the compositor said the surface prefers.

use std::collections::{BTreeMap, HashMap};

use serde_json::{Value, json};

/// The most descriptions kept in `set`: mpv sets one at each change of the picture's colours,
/// a handful a film; past this, a runaway is counted, not kept.
const MOST_SET: usize = 256;

/// A description's settings: each request's name and its arguments, as libwayland prints them.
type Settings = BTreeMap<String, String>;

/// A description mpv created, and what the compositor answered.
#[derive(Clone, Default)]
struct Description {
    settings: Settings,
    answer: Option<String>,
}

/// The colour descriptions mpv set and the one the compositor preferred, as JSON.
pub fn figures(log: &str) -> Value {
    let mut creators: HashMap<String, Settings> = HashMap::new();
    let mut descriptions: HashMap<String, Description> = HashMap::new();
    // What each info object reads, and the description it was asked of.
    let mut information: HashMap<String, Settings> = HashMap::new();
    let mut preferred_descriptions: Vec<String> = Vec::new();
    let mut set: Vec<Value> = Vec::new();
    let mut set_past_most = 0_u64;
    let mut unset = 0_u64;
    let mut failed = 0_u64;
    let mut preferred: Option<Settings> = None;
    for line in log.lines() {
        if let Some((_, request)) = line.split_once(" -> ") {
            let Some((object, name, arguments)) = message(request) else {
                continue;
            };
            match (interface(object), name) {
                ("wp_image_description_creator_params_v1", "create") => {
                    let settings = creators.remove(object).unwrap_or_default();
                    if let Some(id) = new_id(arguments) {
                        descriptions.insert(
                            id.to_owned(),
                            Description {
                                settings,
                                answer: None,
                            },
                        );
                    }
                }
                ("wp_image_description_creator_params_v1", _) => {
                    creators
                        .entry(object.to_owned())
                        .or_default()
                        .insert(name.to_owned(), arguments.to_owned());
                }
                (
                    "wp_color_management_surface_feedback_v1",
                    "get_preferred" | "get_preferred_parametric",
                ) => {
                    if let Some(id) = new_id(arguments) {
                        descriptions.insert(id.to_owned(), Description::default());
                        preferred_descriptions.push(id.to_owned());
                    }
                }
                ("wp_image_description_v1", "get_information") => {
                    if let Some(id) = new_id(arguments)
                        && preferred_descriptions.iter().any(|asked| asked == object)
                    {
                        information.insert(id.to_owned(), Settings::new());
                    }
                }
                ("wp_color_management_surface_v1", "set_image_description") => {
                    let id = arguments.split(',').next().unwrap_or_default().trim();
                    let description = descriptions.get(id).cloned().unwrap_or_default();
                    if set.len() < MOST_SET {
                        let mut entry: serde_json::Map<String, Value> = description
                            .settings
                            .into_iter()
                            .map(|(name, arguments)| (name, json!(arguments)))
                            .collect();
                        entry.insert("answer".to_owned(), json!(description.answer));
                        set.push(Value::Object(entry));
                    } else {
                        set_past_most += 1;
                    }
                }
                ("wp_color_management_surface_v1", "unset_image_description") => unset += 1,
                _ => {}
            }
            continue;
        }
        // Events: `{Default Queue} wp_image_description_v1#46.ready2(0, 3)`.
        let Some(start) = line.find("wp_image_description") else {
            continue;
        };
        let Some((object, name, arguments)) = message(&line[start..]) else {
            continue;
        };
        match (interface(object), name) {
            ("wp_image_description_v1", "ready" | "ready2") => {
                if let Some(description) = descriptions.get_mut(object) {
                    description.answer = Some("ready".to_owned());
                }
            }
            ("wp_image_description_v1", "failed") => {
                failed += 1;
                if let Some(description) = descriptions.get_mut(object) {
                    description.answer = Some(format!("failed({arguments})"));
                }
            }
            ("wp_image_description_info_v1", "done") => {
                if let Some(read) = information.remove(object) {
                    preferred = Some(read);
                }
            }
            ("wp_image_description_info_v1", _) => {
                if let Some(read) = information.get_mut(object) {
                    read.insert(name.to_owned(), arguments.to_owned());
                }
            }
            _ => {}
        }
    }
    json!({
        "set": set,
        "set_past_most": set_past_most,
        "unset": unset,
        "failed": failed,
        "preferred": preferred,
    })
}

/// A message `interface#id.name(arguments)`, split; `None` for anything else.
fn message(text: &str) -> Option<(&str, &str, &str)> {
    let (object, rest) = text.split_once('.')?;
    let (name, arguments) = rest.split_once('(')?;
    Some((object.trim(), name, arguments.trim_end().strip_suffix(')')?))
}

/// The interface of `interface#id`.
fn interface(object: &str) -> &str {
    object
        .split_once('#')
        .map_or(object, |(interface, _)| interface)
}

/// The object a `new id interface#id` argument creates.
fn new_id(arguments: &str) -> Option<&str> {
    let start = arguments.find("new id ")? + "new id ".len();
    let rest = &arguments[start..];
    Some(rest.split([',', ' ']).next().unwrap_or(rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lines mpv prints for one description set on its surface, with libwayland 1.24's
    /// layout, answered `answer` by the compositor.
    fn described(creator: u32, description: u32, answer: &str) -> Vec<String> {
        let request = |text: String| format!("[09:12:01.000001] {{Default Queue}}  -> {text}");
        vec![
            request(format!(
                "wp_color_manager_v1#20.create_parametric_creator(new id wp_image_description_creator_params_v1#{creator})"
            )),
            request(format!(
                "wp_image_description_creator_params_v1#{creator}.set_primaries_named(9)"
            )),
            request(format!(
                "wp_image_description_creator_params_v1#{creator}.set_tf_named(11)"
            )),
            request(format!(
                "wp_image_description_creator_params_v1#{creator}.set_luminances(0, 10000, 203)"
            )),
            request(format!(
                "wp_image_description_creator_params_v1#{creator}.set_max_cll(1000)"
            )),
            request(format!(
                "wp_image_description_creator_params_v1#{creator}.create(new id wp_image_description_v1#{description})"
            )),
            format!(
                "[09:12:01.000300] {{wayland color queue}} wp_image_description_v1#{description}.{answer}"
            ),
            request(format!(
                "wp_color_management_surface_v1#22.set_image_description(wp_image_description_v1#{description}, 0)"
            )),
        ]
    }

    #[test]
    fn each_description_set_keeps_its_settings_and_the_compositors_answer() {
        let mut lines = described(30, 31, "ready2(0, 7)");
        lines.extend(described(30, 31, "failed(1, \"unsupported\")"));
        lines.push("[09:12:02.000000] {Default Queue}  -> wp_color_management_surface_v1#22.unset_image_description()".to_owned());
        let figures = figures(&lines.join("\n"));
        assert_eq!(figures["set"][0]["set_tf_named"], "11");
        assert_eq!(figures["set"][0]["set_luminances"], "0, 10000, 203");
        assert_eq!(figures["set"][0]["answer"], "ready");
        assert_eq!(figures["set"][1]["answer"], "failed(1, \"unsupported\")");
        assert_eq!(figures["failed"], 1);
        assert_eq!(figures["unset"], 1);
    }

    #[test]
    fn the_preferred_description_is_the_last_one_read_whole() {
        let lines = [
            "[09:12:00.1] {Default Queue}  -> wp_color_manager_v1#20.get_surface_feedback(new id wp_color_management_surface_feedback_v1#21, wl_surface#8)",
            "[09:12:00.2] {Default Queue}  -> wp_color_management_surface_feedback_v1#21.get_preferred(new id wp_image_description_v1#40)",
            "[09:12:00.3] {Default Queue}  -> wp_image_description_v1#40.get_information(new id wp_image_description_info_v1#41)",
            "[09:12:00.4] {queue} wp_image_description_info_v1#41.primaries_named(9)",
            "[09:12:00.4] {queue} wp_image_description_info_v1#41.tf_named(11)",
            "[09:12:00.4] {queue} wp_image_description_info_v1#41.luminances(50, 10000, 203)",
            "[09:12:00.5] {queue} wp_image_description_info_v1#41.done()",
            // Another description's information, not the preferred one's, is not read.
            "[09:12:00.6] {Default Queue}  -> wp_image_description_v1#50.get_information(new id wp_image_description_info_v1#51)",
            "[09:12:00.7] {queue} wp_image_description_info_v1#51.tf_named(1)",
            "[09:12:00.8] {queue} wp_image_description_info_v1#51.done()",
        ];
        let figures = figures(&lines.join("\n"));
        assert_eq!(figures["preferred"]["tf_named"], "11");
        assert_eq!(figures["preferred"]["luminances"], "50, 10000, 203");
        assert_eq!(figures["set"], json!([]));
    }

    #[test]
    fn a_log_without_colour_management_has_no_description() {
        let figures =
            figures("[09:12:00.1] {Default Queue}  -> wl_surface#8.commit()\nnot a message");
        assert_eq!(figures["set"], json!([]));
        assert_eq!(figures["preferred"], Value::Null);
        assert_eq!(figures["failed"], 0);
    }
}
