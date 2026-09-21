//! Stable 128-bit identities for markups, scales and viewports.
//!
//! A markup's identity never depends on where its annotation sits in a page's
//! /Annots array, which changes whenever anything before it is deleted. Ours
//! are random (UUID version 4) and written to /NM as hyphenated lowercase hex.
//! Annotations from other programs carry /NM strings of any form, or none, so
//! those get a fresh ID and keep their own /NM unchanged beside it.

use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident, $prefix:literal) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        pub struct $name(pub u128);

        impl $name {
            /// A new random ID.
            pub fn new() -> $name {
                $name(Uuid::new_v4().as_u128())
            }

            /// The ID as written to /NM, with its kind in front so the three
            /// kinds can't be mistaken for each other in a file.
            pub fn to_nm(self) -> String {
                format!(concat!($prefix, "{}"), Uuid::from_u128(self.0).hyphenated())
            }

            /// An ID this app wrote, read back from /NM. `None` for any other
            /// string, including another kind of ID.
            pub fn from_nm(nm: &str) -> Option<$name> {
                let uuid = nm.strip_prefix($prefix)?;
                // Only the exact form written, so a foreign /NM that happens
                // to parse some other way is never taken for one of ours.
                (uuid.len() == 36).then(|| Uuid::try_parse(uuid).ok()).flatten().map(|u| $name(u.as_u128()))
            }
        }

        impl Default for $name {
            fn default() -> $name {
                $name::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str(&self.to_nm())
            }
        }
    };
}

id_type!(
    /// A markup, written to its annotation's /NM.
    MarkupId,
    "KPDF-"
);
id_type!(
    /// A scale, written to the /NM of its /Measure dictionary's /KPDF entry.
    ScaleId,
    "KPDF-SC-"
);
id_type!(
    /// A viewport, written to its /VP entry's /NM.
    ViewportId,
    "KPDF-VP-"
);

/// Zero-based page number.
pub type PageIndex = u32;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_survive_the_file_and_are_not_mistaken_for_each_other() {
        let id = MarkupId::new();
        assert_eq!(MarkupId::from_nm(&id.to_nm()), Some(id));
        assert_ne!(MarkupId::new(), id);
        let scale = ScaleId::new();
        assert_eq!(MarkupId::from_nm(&scale.to_nm()), None);
        assert_eq!(ScaleId::from_nm(&scale.to_nm()), Some(scale));
        assert_eq!(ViewportId::from_nm(&scale.to_nm()), None);
    }

    #[test]
    fn foreign_names_are_not_ours() {
        assert_eq!(MarkupId::from_nm("GXFVLUCUAQJYVCCQ"), None);
        assert_eq!(MarkupId::from_nm("KPDF-not-a-uuid"), None);
        assert_eq!(MarkupId::from_nm("KPDF-6ba7b8109dad11d180b400c04fd430c8"), None, "only the hyphenated form");
        assert_eq!(MarkupId::from_nm(""), None);
    }
}
