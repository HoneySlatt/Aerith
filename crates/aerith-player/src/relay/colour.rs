// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Colour management ([ADR-0024]): mpv's colour descriptions, checked and made again on the
//! window's connection, and the plane's preferred description passed back to mpv.
//!
//! **What mpv is offered.** `wp_color_manager_v1`, at the version the relay bound on the window's
//! connection, 2 at most, when the compositor advertises parametric descriptions ([`Offer`]):
//! parametric creators, surface objects and their feedback, the render intents, named transfer
//! functions and named primaries the compositor advertises, `set_luminances` and the mastering
//! display when it advertises them. Never ICC profiles, which pass a file the relay cannot check,
//! custom primaries, power curves, Windows' scRGB or BT.2100, an output's description, nor the
//! transfer functions `srgb` and `ext_srgb`: Mesa, in mpv's process, describes an sRGB swapchain's
//! surface itself only when the compositor lists `srgb`, and a second description object on mpv's
//! surface would be a protocol error. Requests for what is not offered end mpv's connection.
//!
//! **Each binding sees its own version.** mpv binds version 2, Mesa version 1, on one connection.
//! `compound_power_2_4` and the intent `absolute_no_adaptation` go to version-2 bindings only;
//! `ready` and `preferred_changed` are sent in the form each binding knows, and a description's
//! information is translated to it ([`InfoTarget`]).
//!
//! **Descriptions.** A parametric creator's settings stay in the relay, each checked as it comes
//! against the union of the compositors' refusals ([`check::parametric_set`]). At `create`, the
//! relay makes a creator on the window's connection with the same settings, and creates there; the
//! compositor's `ready` or `failed` goes back to mpv, and a failure is counted. A description mpv
//! asks of its surface's feedback is the compositor's preferred description for the plane's
//! surface, asked on the plane's own feedback object. mpv asks for its information at once, which
//! the relay holds until the compositor said the description is ready: a compositor that never
//! answers `get_preferred` leaves mpv waiting (wlroots, Umbriel and KWin answer at once).
//!
//! **Surfaces.** A description mpv sets on its surface, or unsets, is double-buffered: it reaches
//! the plane's own colour-management surface with the commit it came with
//! ([`super::surface`]). The description made on the window's connection is shared, and lives
//! until mpv's object and every surface state holding it let it go: mpv destroys its description
//! as soon as it set it, before the commit that the relay issues it with. mpv's other surfaces
//! keep their descriptions, checked, and never shown.
//!
//! [ADR-0024]: https://github.com/HoneySlatt/Aerith/blob/main/docs/adr/0024-the-relay-passes-mpvs-colour-description.md

use std::collections::HashMap;
use std::mem;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use wayland_client::{Connection, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::wp::color_management::v1::client::wp_color_management_surface_feedback_v1::{
    self as host_feedback, WpColorManagementSurfaceFeedbackV1 as HostFeedback,
};
use wayland_protocols::wp::color_management::v1::client::wp_color_management_surface_v1::WpColorManagementSurfaceV1 as HostColourSurface;
use wayland_protocols::wp::color_management::v1::client::wp_color_manager_v1::{
    self as host_manager, WpColorManagerV1 as HostManager,
};
use wayland_protocols::wp::color_management::v1::client::wp_image_description_creator_params_v1::WpImageDescriptionCreatorParamsV1 as HostCreator;
use wayland_protocols::wp::color_management::v1::client::wp_image_description_info_v1::{
    self as host_info, WpImageDescriptionInfoV1 as HostInfo,
};
use wayland_protocols::wp::color_management::v1::client::wp_image_description_v1::{
    self as host_description, WpImageDescriptionV1 as HostImageDescription,
};
use wayland_protocols::wp::color_management::v1::server::wp_color_management_surface_feedback_v1::{
    self, WpColorManagementSurfaceFeedbackV1 as MpvFeedback,
};
use wayland_protocols::wp::color_management::v1::server::wp_color_management_surface_v1::{
    self, WpColorManagementSurfaceV1 as MpvColourSurface,
};
use wayland_protocols::wp::color_management::v1::server::wp_color_manager_v1::{
    self, Feature, Primaries, RenderIntent, TransferFunction, WpColorManagerV1 as MpvManager,
};
use wayland_protocols::wp::color_management::v1::server::wp_image_description_creator_params_v1::{
    self, WpImageDescriptionCreatorParamsV1 as MpvCreator,
};
use wayland_protocols::wp::color_management::v1::server::wp_image_description_info_v1::WpImageDescriptionInfoV1 as MpvInfo;
use wayland_protocols::wp::color_management::v1::server::wp_image_description_v1::{
    self, Cause, WpImageDescriptionV1 as MpvDescription,
};
use wayland_server::backend::{ClientId, ObjectId};
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource};

use super::check::{self, ColourOffer, CreatorErrors, Parametric, ParametricSet, holds};
use super::serve::admit;
use super::{Host, State};

/// The highest version of `wp_color_manager_v1` the relay speaks, on both sides: mpv 0.42 binds 2
/// at most.
pub(super) const VERSION: u32 = 2;

/// Whether mpv is offered `set_luminances` when the compositor advertises it. On Umbriel, a
/// description with luminances ends the plane's scanout, and mpv sends them whenever they are
/// offered ([ADR-0024]): the owner decides on a measure whether mpv keeps them. `false` withholds
/// them.
///
/// [ADR-0024]: https://github.com/HoneySlatt/Aerith/blob/main/docs/adr/0024-the-relay-passes-mpvs-colour-description.md
pub(super) const OFFER_SET_LUMINANCES: bool = true;

/// The values of the protocol's enums the relay passes on, a bit for each: every render intent of
/// version 1; every named transfer function but `srgb` (9) and `ext_srgb` (10), and
/// `compound_power_2_4` (14), which version 1 does not know; every named primaries.
const INTENTS_OF_VERSION_1: u32 = 0b1_1111;
const ABSOLUTE_NO_ADAPTATION: u32 = 5;
const TRANSFER_FUNCTIONS_OF_VERSION_1: u32 = 0b11_1001_1111_1110;
const COMPOUND_POWER_2_4: u32 = 14;
const SRGB: u32 = 9;
const NAMED_PRIMARIES: u32 = 0b111_1111_1110;

/// The most events of one description's information passed to mpv: each kind is sent once, and
/// there are nine. A compositor that sends more is not followed.
const MOST_INFORMATION_EVENTS: usize = 16;

/// The longest message of a compositor's `failed` passed to mpv, in bytes.
const LONGEST_FAILURE_MESSAGE: usize = 256;

/// The window's compositor's colour manager, bound by the relay, and what it advertised.
pub(super) struct HostColourManager {
    pub(super) manager: HostManager,
    pub(super) advertised: Advertised,
}

/// What the window's compositor advertised of colour management, a bit for each value, and
/// whether it said all of it (`done`). Values past the protocol's enums are not kept.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Advertised {
    pub(super) intents: u32,
    pub(super) features: u32,
    pub(super) transfer_functions: u32,
    pub(super) primaries: u32,
    pub(super) done: bool,
}

/// What a binding of mpv's is offered, whatever its version: the compositor's list, and whether
/// `set_luminances` is passed on. Each binding keeps it, and reads its own version from it.
#[derive(Clone, Copy, Debug)]
pub(super) struct Offer {
    pub(super) advertised: Advertised,
    pub(super) set_luminances: bool,
}

impl Offer {
    /// Whether the global is offered at all: the compositor said its whole list, with parametric
    /// descriptions, the one kind of description the relay passes on.
    pub(super) fn offered(advertised: &Advertised) -> bool {
        advertised.done && holds(advertised.features, u32::from(Feature::Parametric))
    }

    /// What a binding at `version` is offered.
    pub(super) fn for_version(&self, version: u32) -> ColourOffer {
        let advertised = &self.advertised;
        let mut intents = INTENTS_OF_VERSION_1;
        let mut transfer_functions = TRANSFER_FUNCTIONS_OF_VERSION_1;
        if version >= 2 {
            intents |= 1 << ABSOLUTE_NO_ADAPTATION;
            transfer_functions |= 1 << COMPOUND_POWER_2_4;
        }
        let feature = |feature: Feature| holds(advertised.features, u32::from(feature));
        ColourOffer {
            version,
            intents: intents & advertised.intents,
            transfer_functions: transfer_functions & advertised.transfer_functions,
            primaries: NAMED_PRIMARIES & advertised.primaries,
            set_luminances: self.set_luminances && feature(Feature::SetLuminances),
            mastering: feature(Feature::SetMasteringDisplayPrimaries),
        }
    }
}

/// The values a set holds, a bit for each.
fn values(set: u32) -> impl Iterator<Item = u32> {
    (0..32).filter(move |value| holds(set, *value))
}

/// mpv's colour-management objects, as far as the relay needs them.
pub(super) struct Colour {
    /// mpv's parametric creators: what each binding was offered, and what each set so far.
    creators: HashMap<ObjectId, Creator>,
    descriptions: HashMap<ObjectId, Description>,
    /// mpv's feedback objects, each with the surface it was made for.
    feedbacks: Vec<(MpvFeedback, ObjectId)>,
    /// The descriptions the compositor failed, for the bench: mpv then keeps its target, and its
    /// picture may be wrong.
    pub(super) failed: u64,
    /// The number of the last description made on the window's connection.
    serial: u64,
    /// Whether mpv is offered `set_luminances`: [`OFFER_SET_LUMINANCES`], which the tests turn.
    pub(super) offer_set_luminances: bool,
}

struct Creator {
    offer: ColourOffer,
    description: Parametric,
}

/// One of mpv's image descriptions.
struct Description {
    host: Arc<HostDescription>,
    readiness: Readiness,
    /// Whether it is the compositor's preferred description, whose information mpv may read; the
    /// protocol gives none of mpv's own.
    preferred: bool,
    /// Information mpv asked for before the compositor said the description was ready: mpv 0.42
    /// asks right after `get_preferred`, which a compositor answers at once, and a relay later.
    waiting_information: Vec<MpvInfo>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Readiness {
    Waiting,
    Ready,
    Failed,
}

/// An image description made on the window's connection, destroyed there when its last holder lets
/// it go: mpv's description, or a surface state of mpv's that holds it until the relay issued it.
pub(super) struct HostDescription {
    proxy: HostImageDescription,
    /// Tells it apart from the others, to know whether the plane holds it already.
    serial: u64,
}

impl Drop for HostDescription {
    fn drop(&mut self) {
        // Once the plane is closed, the relay's handle on the window's connection is gone and this
        // sends nothing.
        self.proxy.destroy();
    }
}

/// A description set on one of mpv's surfaces, with its render intent.
#[derive(Clone)]
pub(super) struct SetDescription {
    description: Arc<HostDescription>,
    intent: u32,
}

impl SetDescription {
    /// What tells two settings apart.
    pub(super) fn key(&self) -> (u64, u32) {
        (self.description.serial, self.intent)
    }

    /// Issues it on the plane's colour-management surface. Its intent was offered, so the client
    /// library knows it.
    pub(super) fn issue(&self, surface: &HostColourSurface) {
        if let Ok(intent) = host_manager::RenderIntent::try_from(self.intent) {
            surface.set_image_description(&self.description.proxy, intent);
        }
    }
}

/// A colour-management surface object of mpv's: its surface, and what its binding was offered.
pub(super) struct SurfaceTarget {
    surface: ObjectId,
    offer: ColourOffer,
}

/// One of mpv's information objects, which the compositor's own for the same description feeds.
pub(super) struct InfoTarget {
    mpv: MpvInfo,
    /// The events passed on so far, `done` apart.
    passed: AtomicUsize,
}

impl Colour {
    pub(super) fn new() -> Self {
        Self {
            creators: HashMap::new(),
            descriptions: HashMap::new(),
            feedbacks: Vec::new(),
            failed: 0,
            serial: 0,
            offer_set_luminances: OFFER_SET_LUMINANCES,
        }
    }

    /// Lets every description go: the plane closes.
    pub(super) fn clear(&mut self) {
        self.creators.clear();
        self.descriptions.clear();
        self.feedbacks.clear();
    }

    fn keep(&mut self, mpv: &MpvDescription, host: HostImageDescription, preferred: bool) {
        self.serial += 1;
        let host = Arc::new(HostDescription {
            proxy: host,
            serial: self.serial,
        });
        self.descriptions.insert(
            mpv.id(),
            Description {
                host,
                readiness: Readiness::Waiting,
                preferred,
                waiting_information: Vec::new(),
            },
        );
    }
}

impl Host {
    /// The plane surface's colour-management surface, made when first needed.
    pub(super) fn plane_colour_surface(
        &mut self,
        qh: &QueueHandle<State>,
    ) -> Option<&HostColourSurface> {
        if self.colour_surface.is_none() {
            let manager = &self.globals.colour.as_ref()?.manager;
            self.colour_surface = Some(manager.get_surface(&self.surface, qh, ()));
        }
        self.colour_surface.as_ref()
    }

    /// The plane surface's colour feedback, made when first needed.
    fn plane_colour_feedback(&mut self, qh: &QueueHandle<State>) -> Option<&HostFeedback> {
        if self.colour_feedback.is_none() {
            let manager = &self.globals.colour.as_ref()?.manager;
            self.colour_feedback = Some(manager.get_surface_feedback(&self.surface, qh, ()));
        }
        self.colour_feedback.as_ref()
    }
}

impl GlobalDispatch<MpvManager, Advertised> for State {
    fn bind(
        state: &mut Self,
        _: &DisplayHandle,
        client: &Client,
        resource: New<MpvManager>,
        advertised: &Advertised,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let offer = Offer {
            advertised: *advertised,
            set_luminances: state.colour.offer_set_luminances,
        };
        let Some(manager) = admit(state, client, resource, offer, data_init) else {
            return;
        };
        let offered = offer.for_version(manager.version());
        for intent in values(offered.intents) {
            if let Ok(intent) = RenderIntent::try_from(intent) {
                manager.supported_intent(intent);
            }
        }
        manager.supported_feature(Feature::Parametric);
        if offered.set_luminances {
            manager.supported_feature(Feature::SetLuminances);
        }
        if offered.mastering {
            manager.supported_feature(Feature::SetMasteringDisplayPrimaries);
        }
        for tf in values(offered.transfer_functions) {
            if let Ok(tf) = TransferFunction::try_from(tf) {
                manager.supported_tf_named(tf);
            }
        }
        for primaries in values(offered.primaries) {
            if let Ok(primaries) = Primaries::try_from(primaries) {
                manager.supported_primaries_named(primaries);
            }
        }
        manager.done();
    }
}

impl Dispatch<MpvManager, Offer> for State {
    fn request(
        state: &mut Self,
        client: &Client,
        manager: &MpvManager,
        request: wp_color_manager_v1::Request,
        offer: &Offer,
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        use wp_color_manager_v1::{Error, Request};
        let not_offered = |what: &str| {
            manager.post_error(
                Error::UnsupportedFeature,
                format!("wp_color_manager_v1.{what} is not offered"),
            );
        };
        match request {
            Request::GetSurface { id, surface } => {
                let surface = surface.id();
                // As wlroots: "wp_color_management_surface_v1 already constructed for this
                // surface".
                if state
                    .surfaces
                    .get(&surface)
                    .is_some_and(|record| record.has_colour_surface)
                {
                    manager.post_error(
                        Error::SurfaceExists,
                        "the surface has a colour-management surface already",
                    );
                    return;
                }
                let target = SurfaceTarget {
                    surface: surface.clone(),
                    offer: offer.for_version(manager.version()),
                };
                if admit(state, client, id, target, data_init).is_some()
                    && let Some(record) = state.surfaces.get_mut(&surface)
                {
                    record.has_colour_surface = true;
                }
            }
            Request::GetSurfaceFeedback { id, surface } => {
                let Some(feedback) = admit(state, client, id, surface.id(), data_init) else {
                    return;
                };
                // mpv's other surfaces are never shown: every feedback follows the plane's.
                state.host.plane_colour_feedback(&state.qh);
                state.colour.feedbacks.push((feedback, surface.id()));
            }
            Request::CreateParametricCreator { obj } => {
                let Some(creator) = admit(state, client, obj, (), data_init) else {
                    return;
                };
                state.colour.creators.insert(
                    creator.id(),
                    Creator {
                        offer: offer.for_version(manager.version()),
                        description: Parametric::default(),
                    },
                );
            }
            // wlroots: "new_icc_creator is not supported"; an ICC profile passes a file the relay
            // cannot check.
            Request::CreateIccCreator { .. } => not_offered("create_icc_creator"),
            Request::CreateWindowsScrgb { .. } => not_offered("create_windows_scrgb"),
            Request::CreateWindowsBt2100 { .. } => not_offered("create_windows_bt2100"),
            // Neither has a feature of its own: the protocol's code for what is not supported is
            // the closest. Umbriel refuses `get_image_description` too.
            Request::GetOutput { .. } => not_offered("get_output"),
            Request::GetImageDescription { .. } => not_offered("get_image_description"),
            _ => {}
        }
    }

    fn destroyed(state: &mut Self, _: ClientId, _: &MpvManager, _: &Offer) {
        state.object_gone();
    }
}

/// The error codes of `wp_image_description_creator_params_v1`, for the checks.
fn creator_errors() -> CreatorErrors {
    use wp_image_description_creator_params_v1::Error;
    CreatorErrors {
        incomplete_set: Error::IncompleteSet.into(),
        already_set: Error::AlreadySet.into(),
        unsupported_feature: Error::UnsupportedFeature.into(),
        invalid_tf: Error::InvalidTf.into(),
        invalid_primaries_named: Error::InvalidPrimariesNamed.into(),
        invalid_luminance: Error::InvalidLuminance.into(),
    }
}

/// The raw value of an enum mpv or the compositor sent.
fn raw<T: Into<u32>>(value: WEnum<T>) -> u32 {
    match value {
        WEnum::Value(value) => value.into(),
        WEnum::Unknown(value) => value,
    }
}

/// mpv's parametric creator: each setting is checked and kept, and nothing reaches the window's
/// connection before `create`.
impl Dispatch<MpvCreator, ()> for State {
    fn request(
        state: &mut Self,
        client: &Client,
        creator: &MpvCreator,
        request: wp_image_description_creator_params_v1::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        use wp_image_description_creator_params_v1::Request;
        let set = match request {
            Request::Create { image_description } => {
                return state.create_description(client, creator, image_description, data_init);
            }
            Request::SetTfNamed { tf } => ParametricSet::TransferFunction(raw(tf)),
            Request::SetTfPower { .. } => ParametricSet::TransferPower,
            Request::SetPrimariesNamed { primaries } => ParametricSet::Primaries(raw(primaries)),
            Request::SetPrimaries { .. } => ParametricSet::PrimaryCoordinates,
            Request::SetLuminances {
                min_lum,
                max_lum,
                reference_lum,
            } => ParametricSet::Luminances {
                min: min_lum,
                max: max_lum,
                reference: reference_lum,
            },
            Request::SetMasteringDisplayPrimaries {
                r_x,
                r_y,
                g_x,
                g_y,
                b_x,
                b_y,
                w_x,
                w_y,
            } => ParametricSet::MasteringPrimaries([r_x, r_y, g_x, g_y, b_x, b_y, w_x, w_y]),
            Request::SetMasteringLuminance { min_lum, max_lum } => {
                ParametricSet::MasteringLuminance {
                    min: min_lum,
                    max: max_lum,
                }
            }
            Request::SetMaxCll { max_cll } => ParametricSet::MaxCll(max_cll),
            Request::SetMaxFall { max_fall } => ParametricSet::MaxFall(max_fall),
            _ => return,
        };
        let Some(record) = state.colour.creators.get_mut(&creator.id()) else {
            return;
        };
        if let Err(refusal) = check::parametric_set(
            &mut record.description,
            set,
            &record.offer,
            creator_errors(),
        ) {
            creator.post_error(refusal.code, refusal.message);
        }
    }

    fn destroyed(state: &mut Self, _: ClientId, creator: &MpvCreator, _: &()) {
        state.object_gone();
        state.colour.creators.remove(&creator.id());
    }
}

impl State {
    /// mpv's `create`: the description is checked whole, then made on the window's connection
    /// with a creator of the relay's, its settings issued in one go.
    fn create_description(
        &mut self,
        client: &Client,
        creator: &MpvCreator,
        id: New<MpvDescription>,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let errors = creator_errors();
        let checked = match self.colour.creators.get(&creator.id()) {
            Some(record) => {
                check::parametric_create(&record.description, record.offer.version, errors)
                    .map(|()| record.description)
            }
            // A creator the relay does not know cannot be made: refused, as its new object must
            // be made or refused.
            None => Err(check::Refusal::new(
                errors.incomplete_set,
                "an unknown creator",
            )),
        };
        let description = match checked {
            Ok(description) => description,
            Err(refusal) => return creator.post_error(refusal.code, refusal.message),
        };
        // Every value was offered, so the client library knows it.
        let (Some(tf), Some(primaries)) = (
            description
                .transfer_function
                .and_then(|tf| host_manager::TransferFunction::try_from(tf).ok()),
            description
                .primaries
                .and_then(|primaries| host_manager::Primaries::try_from(primaries).ok()),
        ) else {
            return creator.post_error(errors.invalid_tf, "a value the relay cannot pass on");
        };
        let Some(manager) = self
            .host
            .globals
            .colour
            .as_ref()
            .map(|colour| colour.manager.clone())
        else {
            return creator.post_error(errors.unsupported_feature, "no colour manager");
        };
        let Some(mpv) = admit(self, client, id, (), data_init) else {
            return;
        };
        let host_creator: HostCreator = manager.create_parametric_creator(&self.qh, ());
        host_creator.set_tf_named(tf);
        host_creator.set_primaries_named(primaries);
        if let Some((min, max, reference)) = description.luminances {
            host_creator.set_luminances(min, max, reference);
        }
        if let Some([r_x, r_y, g_x, g_y, b_x, b_y, w_x, w_y]) = description.mastering_primaries {
            host_creator.set_mastering_display_primaries(r_x, r_y, g_x, g_y, b_x, b_y, w_x, w_y);
        }
        if let Some((min, max)) = description.mastering_luminance {
            host_creator.set_mastering_luminance(min, max);
        }
        if let Some(max_cll) = description.max_cll {
            host_creator.set_max_cll(max_cll);
        }
        if let Some(max_fall) = description.max_fall {
            host_creator.set_max_fall(max_fall);
        }
        let host = host_creator.create(&self.qh, mpv.clone());
        self.colour.keep(&mpv, host, false);
    }

    /// The compositor said a description is ready: so is mpv's, and the information mpv asked
    /// for meanwhile is asked of the compositor.
    fn description_ready(&mut self, mpv: &MpvDescription, identity: u64) {
        let Some(record) = self.colour.descriptions.get_mut(&mpv.id()) else {
            return;
        };
        if record.readiness != Readiness::Waiting {
            return;
        }
        record.readiness = Readiness::Ready;
        let (high, low) = split(identity);
        if mpv.version() >= 2 {
            mpv.ready2(high, low);
        } else {
            // A version-1 binding takes 32 bits: the low ones. They may be 0, which the protocol
            // reserves, or equal another description's whose identity differs above them; neither
            // is reached in practice, as compositors count identities from 1.
            mpv.ready(low);
        }
        for info in mem::take(&mut record.waiting_information) {
            record.host.proxy.get_information(
                &self.qh,
                InfoTarget {
                    mpv: info,
                    passed: AtomicUsize::new(0),
                },
            );
        }
    }

    /// The compositor failed a description: so does mpv's, with the compositor's cause, and it is
    /// counted.
    fn description_failed(
        &mut self,
        mpv: &MpvDescription,
        cause: WEnum<host_description::Cause>,
        message: &str,
    ) {
        let Some(record) = self.colour.descriptions.get_mut(&mpv.id()) else {
            return;
        };
        if record.readiness != Readiness::Waiting {
            return;
        }
        record.readiness = Readiness::Failed;
        let information_asked = !record.waiting_information.is_empty();
        self.colour.failed += 1;
        let cause = Cause::try_from(raw(cause)).unwrap_or(Cause::Unsupported);
        mpv.failed(cause, bounded(message));
        if information_asked {
            // mpv asked for the information of a description that turned out failed: the
            // protocol's error, as wlroots raises it ("image description is in failed state").
            mpv.post_error(
                wp_image_description_v1::Error::NotReady,
                "get_information on a description that then failed",
            );
        }
    }
}

/// The high and low 32 bits of an identity.
fn split(identity: u64) -> (u32, u32) {
    let low = u32::try_from(identity & u64::from(u32::MAX)).unwrap_or(u32::MAX);
    let high = u32::try_from(identity >> 32).unwrap_or(u32::MAX);
    (high, low)
}

/// A compositor's message, short and without the zero byte that ends a string on the wire.
fn bounded(message: &str) -> String {
    let mut bounded: String = message.chars().filter(|c| *c != '\0').collect();
    if bounded.len() > LONGEST_FAILURE_MESSAGE {
        let mut end = LONGEST_FAILURE_MESSAGE;
        while !bounded.is_char_boundary(end) {
            end -= 1;
        }
        bounded.truncate(end);
    }
    bounded
}

/// mpv's descriptions: only the compositor's preferred one tells its information, once ready.
impl Dispatch<MpvDescription, ()> for State {
    fn request(
        state: &mut Self,
        client: &Client,
        description: &MpvDescription,
        request: wp_image_description_v1::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        use wp_image_description_v1::{Error, Request};
        let Request::GetInformation { information } = request else {
            return;
        };
        let refusal = match state.colour.descriptions.get(&description.id()) {
            // wlroots, first: "image description is in failed state".
            None => Some((Error::NotReady, "get_information on an unknown description")),
            Some(record) if record.readiness == Readiness::Failed => Some((
                Error::NotReady,
                "get_information on a description that failed",
            )),
            // The protocol: "The resulting image description object does not allow
            // get_information request" of a parametric creator's; wlroots "get_information not
            // allowed".
            Some(record) if !record.preferred => Some((
                Error::NoInformation,
                "get_information on mpv's own description",
            )),
            Some(_) => None,
        };
        if let Some((code, message)) = refusal {
            description.post_error(code, message);
            return;
        }
        let Some(info) = admit(state, client, information, (), data_init) else {
            return;
        };
        let Some(record) = state.colour.descriptions.get_mut(&description.id()) else {
            return;
        };
        if record.readiness == Readiness::Ready {
            record.host.proxy.get_information(
                &state.qh,
                InfoTarget {
                    mpv: info,
                    passed: AtomicUsize::new(0),
                },
            );
        } else {
            record.waiting_information.push(info);
        }
    }

    fn destroyed(state: &mut Self, _: ClientId, description: &MpvDescription, _: &()) {
        state.object_gone();
        if let Some(record) = state.colour.descriptions.remove(&description.id()) {
            // Information still waiting will never come: it ends at once, empty.
            for info in record.waiting_information {
                info.done();
            }
        }
    }
}

/// mpv's information objects have no request; each ends with the `done` the relay sends it.
impl Dispatch<MpvInfo, ()> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &MpvInfo,
        _: <MpvInfo as Resource>::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }

    fn destroyed(state: &mut Self, _: ClientId, _: &MpvInfo, _: &()) {
        state.object_gone();
    }
}

/// mpv's colour-management surfaces: what they set is checked, kept as the surface's pending
/// state, and reaches the plane at mpv's commit.
impl Dispatch<MpvColourSurface, SurfaceTarget> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        object: &MpvColourSurface,
        request: wp_color_management_surface_v1::Request,
        target: &SurfaceTarget,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        use wp_color_management_surface_v1::{Error, Request};
        let set = match request {
            Request::SetImageDescription {
                image_description,
                render_intent,
            } => Some((image_description, raw(render_intent))),
            Request::UnsetImageDescription => None,
            _ => return,
        };
        // wlroots: "set_image_description cannot be sent on an inert object", and the same for
        // the unset.
        if !state.surfaces.contains_key(&target.surface) {
            object.post_error(
                Error::Inert,
                "the colour-management surface's surface was destroyed",
            );
            return;
        }
        let change = match set {
            None => None,
            Some((description, intent)) => {
                let ready = state
                    .colour
                    .descriptions
                    .get(&description.id())
                    .filter(|record| record.readiness == Readiness::Ready);
                // wlroots "Image description to be set is invalid", KWin "failed image description
                // can't be used"; the protocol forbids one not ready.
                let Some(record) = ready else {
                    object.post_error(
                        Error::ImageDescription,
                        "set_image_description with a description that is not ready",
                    );
                    return;
                };
                // wlroots "invalid render intent"; Umbriel "unsupported render intent".
                if !holds(target.offer.intents, intent) {
                    object.post_error(
                        Error::RenderIntent,
                        format!("render intent {intent} was not offered"),
                    );
                    return;
                }
                Some(SetDescription {
                    description: Arc::clone(&record.host),
                    intent,
                })
            }
        };
        if let Some(surface) = state.surfaces.get_mut(&target.surface) {
            surface.set_colour(change);
        }
    }

    fn destroyed(state: &mut Self, _: ClientId, _: &MpvColourSurface, target: &SurfaceTarget) {
        state.object_gone();
        // The protocol: destroying it does "the same as unset_image_description".
        if let Some(surface) = state.surfaces.get_mut(&target.surface) {
            surface.has_colour_surface = false;
            surface.set_colour(None);
        }
    }
}

/// mpv's colour feedback: each preferred description it asks for is the plane's, asked of the
/// compositor on the plane's own feedback.
impl Dispatch<MpvFeedback, ObjectId> for State {
    fn request(
        state: &mut Self,
        client: &Client,
        feedback: &MpvFeedback,
        request: wp_color_management_surface_feedback_v1::Request,
        surface: &ObjectId,
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        use wp_color_management_surface_feedback_v1::{Error, Request};
        let (id, parametric) = match request {
            Request::GetPreferred { image_description } => (image_description, false),
            Request::GetPreferredParametric { image_description } => (image_description, true),
            _ => return,
        };
        // wlroots: "…sent on inert feedback surface".
        if !state.surfaces.contains_key(surface) {
            feedback.post_error(Error::Inert, "the colour feedback's surface was destroyed");
            return;
        }
        let Some(mpv) = admit(state, client, id, (), data_init) else {
            return;
        };
        let qh = state.qh.clone();
        let host = state.host.plane_colour_feedback(&qh).map(|plane| {
            if parametric {
                plane.get_preferred_parametric(&qh, mpv.clone())
            } else {
                plane.get_preferred(&qh, mpv.clone())
            }
        });
        match host {
            Some(host) => state.colour.keep(&mpv, host, true),
            // Not reached: mpv is offered colour management only with a colour manager.
            None => mpv.failed(Cause::OperatingSystem, "no colour manager".to_owned()),
        }
    }

    fn destroyed(state: &mut Self, _: ClientId, feedback: &MpvFeedback, _: &ObjectId) {
        state.object_gone();
        state
            .colour
            .feedbacks
            .retain(|(object, _)| object != feedback);
    }
}

impl wayland_client::Dispatch<HostManager, ()> for State {
    fn event(
        state: &mut Self,
        _: &HostManager,
        event: host_manager::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use host_manager::Event;
        let Some(colour) = state.host.globals.colour.as_mut() else {
            return;
        };
        let advertised = &mut colour.advertised;
        let (set, value) = match event {
            Event::SupportedIntent { render_intent } => {
                (&mut advertised.intents, raw(render_intent))
            }
            Event::SupportedFeature { feature } => (&mut advertised.features, raw(feature)),
            Event::SupportedTfNamed { tf } => (&mut advertised.transfer_functions, raw(tf)),
            Event::SupportedPrimariesNamed { primaries } => {
                (&mut advertised.primaries, raw(primaries))
            }
            Event::Done => {
                advertised.done = true;
                return;
            }
            _ => return,
        };
        if value < 32 {
            *set |= 1 << value;
        }
    }
}

impl wayland_client::Dispatch<HostImageDescription, MpvDescription> for State {
    fn event(
        state: &mut Self,
        _: &HostImageDescription,
        event: host_description::Event,
        mpv: &MpvDescription,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use host_description::Event;
        match event {
            Event::Ready { identity } => state.description_ready(mpv, u64::from(identity)),
            Event::Ready2 {
                identity_hi,
                identity_lo,
            } => state
                .description_ready(mpv, (u64::from(identity_hi) << 32) | u64::from(identity_lo)),
            Event::Failed { cause, msg } => state.description_failed(mpv, cause, &msg),
            _ => {}
        }
    }
}

/// The compositor's information on a preferred description, passed to mpv's own information
/// object in the version mpv bound. Its ICC profile is never passed: its file is closed here.
impl wayland_client::Dispatch<HostInfo, InfoTarget> for State {
    fn event(
        _: &mut Self,
        _: &HostInfo,
        event: host_info::Event,
        target: &InfoTarget,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use host_info::Event;
        let mpv = &target.mpv;
        match event {
            Event::Done => {
                mpv.done();
                return;
            }
            // The file is dropped with the event, and so closed.
            Event::IccFile { .. } => return,
            _ => {}
        }
        if target.passed.fetch_add(1, Ordering::Relaxed) >= MOST_INFORMATION_EVENTS {
            return;
        }
        match event {
            Event::Primaries {
                r_x,
                r_y,
                g_x,
                g_y,
                b_x,
                b_y,
                w_x,
                w_y,
            } => mpv.primaries(r_x, r_y, g_x, g_y, b_x, b_y, w_x, w_y),
            Event::PrimariesNamed { primaries } => {
                let named = Some(raw(primaries))
                    .filter(|value| holds(NAMED_PRIMARIES, *value))
                    .and_then(|value| Primaries::try_from(value).ok());
                if let Some(primaries) = named {
                    mpv.primaries_named(primaries);
                }
            }
            Event::TfPower { eexp } => mpv.tf_power(eexp),
            Event::TfNamed { tf } => {
                let named = transfer_function_for(raw(tf), mpv.version())
                    .and_then(|value| TransferFunction::try_from(value).ok());
                if let Some(tf) = named {
                    mpv.tf_named(tf);
                }
            }
            Event::Luminances {
                min_lum,
                max_lum,
                reference_lum,
            } => mpv.luminances(min_lum, max_lum, reference_lum),
            Event::TargetPrimaries {
                r_x,
                r_y,
                g_x,
                g_y,
                b_x,
                b_y,
                w_x,
                w_y,
            } => mpv.target_primaries(r_x, r_y, g_x, g_y, b_x, b_y, w_x, w_y),
            Event::TargetLuminance { min_lum, max_lum } => mpv.target_luminance(min_lum, max_lum),
            Event::TargetMaxCll { max_cll } => mpv.target_max_cll(max_cll),
            Event::TargetMaxFall { max_fall } => mpv.target_max_fall(max_fall),
            _ => {}
        }
    }
}

/// A named transfer function of a description's information, as a binding at `version` knows it:
/// `compound_power_2_4` becomes `srgb` below version 2, as a compositor should send it there
/// (KWin 6.8 sends 14 whatever the version); a value the protocol does not define is dropped.
fn transfer_function_for(tf: u32, version: u32) -> Option<u32> {
    match tf {
        1..=13 => Some(tf),
        COMPOUND_POWER_2_4 if version >= 2 => Some(tf),
        COMPOUND_POWER_2_4 => Some(SRGB),
        _ => None,
    }
}

/// The plane's preferred description changed: every feedback of mpv's is told, in the form its
/// binding knows, but those whose surface is gone.
impl wayland_client::Dispatch<HostFeedback, ()> for State {
    fn event(
        state: &mut Self,
        _: &HostFeedback,
        event: host_feedback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use host_feedback::Event;
        let identity = match event {
            Event::PreferredChanged { identity } => u64::from(identity),
            Event::PreferredChanged2 {
                identity_hi,
                identity_lo,
            } => (u64::from(identity_hi) << 32) | u64::from(identity_lo),
            _ => return,
        };
        let (high, low) = split(identity);
        state
            .colour
            .feedbacks
            .retain(|(feedback, _)| feedback.is_alive());
        for (feedback, surface) in &state.colour.feedbacks {
            if !state.surfaces.contains_key(surface) {
                continue;
            }
            if feedback.version() >= 2 {
                feedback.preferred_changed2(high, low);
            } else {
                // The low 32 bits, as for `ready`: 0 or a collision only past 2³² identities.
                feedback.preferred_changed(low);
            }
        }
    }
}

delegate_noop!(State: HostCreator);
delegate_noop!(State: HostColourSurface);

#[cfg(test)]
mod tests {
    use super::*;

    fn advertised(features: &[Feature], transfer_functions: &[u32]) -> Advertised {
        Advertised {
            intents: 0b11_1111,
            features: features
                .iter()
                .fold(0, |set, feature| set | 1 << u32::from(*feature)),
            transfer_functions: transfer_functions.iter().fold(0, |set, tf| set | 1 << tf),
            primaries: 0b111_1111_1110,
            done: true,
        }
    }

    #[test]
    fn srgb_is_never_offered_and_compound_power_only_to_version_2() {
        let offer = Offer {
            advertised: advertised(&[Feature::Parametric], &(1..=14).collect::<Vec<_>>()),
            set_luminances: true,
        };
        let one = offer.for_version(1).transfer_functions;
        let two = offer.for_version(2).transfer_functions;
        for tf in [9, 10] {
            assert!(!holds(one, tf) && !holds(two, tf), "{tf}");
        }
        assert!(!holds(one, 14));
        assert!(holds(two, 14));
        assert_eq!(values(one).count(), 11);
        assert!(!holds(offer.for_version(1).intents, 5));
        assert!(holds(offer.for_version(2).intents, 5));
    }

    #[test]
    fn set_luminances_is_offered_only_when_the_compositor_has_it_and_the_switch_lets_it() {
        let with = advertised(&[Feature::Parametric, Feature::SetLuminances], &[2]);
        let without = advertised(&[Feature::Parametric], &[2]);
        let offered = |advertised, set_luminances| {
            Offer {
                advertised,
                set_luminances,
            }
            .for_version(2)
            .set_luminances
        };
        assert!(offered(with, true));
        assert!(!offered(without, true));
        assert!(!offered(with, false));
    }

    #[test]
    fn only_a_whole_list_with_parametric_descriptions_is_offered() {
        let mut list = advertised(&[Feature::Parametric], &[2]);
        assert!(Offer::offered(&list));
        list.done = false;
        assert!(!Offer::offered(&list));
        assert!(!Offer::offered(&advertised(&[Feature::IccV2V4], &[2])));
    }

    #[test]
    fn a_transfer_function_is_told_in_the_bindings_version() {
        assert_eq!(transfer_function_for(14, 1), Some(9));
        assert_eq!(transfer_function_for(14, 2), Some(14));
        assert_eq!(transfer_function_for(11, 1), Some(11));
        assert_eq!(transfer_function_for(0, 2), None);
        assert_eq!(transfer_function_for(15, 2), None);
    }

    #[test]
    fn an_identity_splits_into_its_halves() {
        assert_eq!(split((7 << 32) | 9), (7, 9));
        assert_eq!(split(u64::MAX), (u32::MAX, u32::MAX));
    }

    #[test]
    fn a_failure_message_is_bounded_and_has_no_zero_byte() {
        assert_eq!(bounded("a\0b"), "ab");
        let long = "é".repeat(LONGEST_FAILURE_MESSAGE);
        let cut = bounded(&long);
        assert!(cut.len() <= LONGEST_FAILURE_MESSAGE && cut.chars().all(|c| c == 'é'));
    }
}
