//! The UI-agnostic core of fastdash.
//!
//! A connector-agnostic `engine` (the `Connector` trait, generic render
//! `Panel`s, a registry, config, secrets, i18n and the shared fetch path) with
//! self-contained `connectors` plugged in behind that trait, plus the Windows
//! taskbar readout. A UI crate only ever sees `Panel`s, so adding a connector
//! needs zero UI changes.
//!
//! Nothing here fetches on a timer. The UI drives every fetch: the visible tab
//! when it has nothing cached yet, and whatever the user refreshes by hand.

// TODO: remove once every connector is fleshed out; keeps the scaffold quiet.
#![allow(dead_code)]

pub mod connectors;
pub mod engine;
pub mod taskbar;
