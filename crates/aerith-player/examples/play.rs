// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Plays one file or stream through `aerith-player` and prints each event as it comes, with the
//! time since the source was opened, a second after the player started: the first one is the time
//! to the first frame.
//!
//! ```text
//! cargo run --release -p aerith-player --example play -- <file or URL> [--sound]
//!     [--seconds N] [--mpv PROGRAM] [--header "Name: value"]...
//! ```
//!
//! The picture is discarded: showing it needs a window of an interface's, which the test interface,
//! `aerith-testbed` (ADR-0017), gives. By default the sound is discarded too, and nothing is heard;
//! `--sound` plays it. The player stops after `--seconds`, 10 by default. mpv's shader
//! cache goes to a fresh folder under the system's temporary folder, removed at the end.

#[cfg(target_os = "linux")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use aerith_player::{Audio, Event, Header, Player, Settings, Source, Video};
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::{Duration, Instant};

    let mut arguments = std::env::args().skip(1);
    let mut target = None;
    let mut audio = Audio::Discarded;
    let mut seconds = 10.0;
    let mut engine = "mpv".to_owned();
    let mut headers = Vec::new();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--sound" => audio = Audio::System,
            "--seconds" => seconds = arguments.next().ok_or("--seconds needs a value")?.parse()?,
            "--mpv" => engine = arguments.next().ok_or("--mpv needs a program")?,
            "--header" => {
                let line = arguments.next().ok_or("--header needs a value")?;
                let (name, value) = line.split_once(':').ok_or("a header is \"Name: value\"")?;
                headers.push(Header::new(name.trim(), value.trim())?);
            }
            _ if target.is_none() => target = Some(argument),
            _ => return Err(format!("unexpected argument {argument}").into()),
        }
    }
    let target = target.ok_or("usage: play <file or URL> [--sound] [--seconds N]")?;
    let source = if target.starts_with("http://") || target.starts_with("https://") {
        Source::stream(target, headers)?
    } else {
        Source::file(target)?
    };
    let cache = std::env::temp_dir().join(format!("aerith-player-play-{}", std::process::id()));
    let settings = Settings {
        engine: engine.into(),
        video: Video::Discarded,
        audio,
        cache: cache.clone(),
    };

    let (player, events) = Player::start(&settings)?;
    // Aerith starts its player before the person presses Play, as the bench starts each rival
    // before it loads the clip: the time to the first frame does not include mpv's own start.
    std::thread::sleep(Duration::from_secs(1));
    let opened = Instant::now();
    player.open(&source)?;
    let deadline = opened + Duration::from_secs_f64(seconds);
    loop {
        let event = match events.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(event) => event,
            Err(RecvTimeoutError::Timeout) => break,
            Err(RecvTimeoutError::Disconnected) => return Err("the player stopped".into()),
        };
        let since = opened.elapsed().as_secs_f64() * 1000.0;
        println!("{since:9.1} ms  {event:?}");
        if matches!(event, Event::Ended(..) | Event::EngineFailed(_)) {
            break;
        }
    }
    drop(player);
    let _ = std::fs::remove_dir_all(cache);
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("aerith-player has no engine on this system yet: Windows joins at milestone M1.6");
}
