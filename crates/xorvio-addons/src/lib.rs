//! Add-ons that speak the Stremio add-on protocol: rows of titles to browse,
//! search, a title's details and episodes, its copies to play and subtitles.
//!
//! Xorvio ships with no add-ons and favours none: people paste a manifest link
//! and every add-on is treated the same way. Links can carry account keys, so
//! they're kept only in the [`SecretStore`]; `addons.json` and everything the
//! interface sees name an add-on by its host alone, and errors never include a
//! link.
//!
//! Add-on titles become the shared [`xorvio_model::Movie`],
//! [`xorvio_model::Series`] and [`xorvio_model::Episode`] types with
//! `source_id = "addon"`, ids `addon|<type>|<meta id>` and
//! [`xorvio_model::StreamLocator::Addon`].
//!
//! Protocol reference: <https://github.com/Stremio/stremio-addon-sdk/tree/master/docs/api>

mod addons;
mod client;
mod labels;
mod lenient;
mod mapping;
mod protocol;
mod registry;
mod secrets;
mod subtitle_text;

pub use addons::Addons;
pub use client::AddonError;
pub use mapping::{SOURCE_ID, item_id, parse_item_id};
pub use registry::secret_key;
pub use secrets::{MemorySecrets, SecretStore};
pub use subtitle_text::to_webvtt;
