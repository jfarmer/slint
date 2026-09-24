// Copyright © SixtyFPS GmbH <info@slint.dev>
// SPDX-License-Identifier: GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0

pub use fontique;
pub use skrifa;

#[cfg(feature = "svg-text")]
pub mod svg;

#[cfg(any(target_family = "wasm", target_os = "nto"))]
use fontique::ScriptExt;

use std::collections::HashSet;
use std::sync::Arc;

/// Create a new fontique Collection.
/// When `shared` is true, the collection uses `Arc`-based internal sharing,
/// so that clones share the underlying data and mutations are visible across clones.
pub fn create_collection(shared: bool) -> Collection {
    let mut collection = fontique::Collection::new(fontique::CollectionOptions {
        shared,
        system_fonts: std::env::var("SLINT_NO_SYSTEM_FONTS").as_deref() != Ok("1"),
    });
    let mut source_cache =
        if shared { fontique::SourceCache::new_shared() } else { fontique::SourceCache::default() };

    // Preserves insertion order — the primary (SLINT_DEFAULT_FONT) lands first, fallbacks
    // (SLINT_FONT_PATH) follow. The runtime bitmap-font fallback and the compile-time
    // bitmap-font emission both rely on this ordering rather than any later sort.
    let mut default_fonts: Vec<(std::path::PathBuf, fontique::QueryFont)> = Vec::new();
    let mut chain_families: Vec<fontique::FamilyId> = Vec::new();

    #[cfg(any(target_family = "wasm", target_os = "nto"))]
    {
        let data = include_bytes!("sharedfontique/Inter-VariableFont.ttf");
        let fonts = collection.register_fonts(fontique::Blob::new(Arc::new(data)), None);
        for script in fontique::Script::all_samples().iter().map(|(script, _)| *script) {
            collection.append_fallbacks(
                fontique::FallbackKey::new(script, None),
                fonts.iter().map(|(family_id, _)| *family_id),
            );
        }
        for generic_family in [
            fontique::GenericFamily::SansSerif,
            fontique::GenericFamily::SystemUi,
            fontique::GenericFamily::UiSansSerif,
        ] {
            collection.append_generic_families(
                generic_family,
                fonts.iter().map(|(family_id, _)| *family_id),
            );
        }
    }

    let mut registered_paths: HashSet<std::path::PathBuf> = HashSet::new();
    let mut register_path =
        |path: std::path::PathBuf,
         collection: &mut fontique::Collection,
         source_cache: &mut fontique::SourceCache,
         default_fonts: &mut Vec<_>,
         chain_families: &mut Vec<fontique::FamilyId>| {
            if !registered_paths.insert(path.clone()) {
                return;
            }
            let Ok(bytes) = std::fs::read(&path) else { return };
            let fonts = collection.register_fonts(bytes.into(), None);
            if fonts.is_empty() {
                return;
            }
            for (family_id, _) in &fonts {
                if !chain_families.contains(family_id) {
                    chain_families.push(*family_id);
                }
            }
            if let Some(font) = fonts.first().and_then(|(id, infos)| {
                let info = infos.first()?;
                get_font_for_info(collection, source_cache, *id, info)
            }) {
                default_fonts.push((path, font));
            }
        };

    // SLINT_DEFAULT_FONT: a single .ttf to act as the primary font.
    if let Some(path) = std::env::var_os("SLINT_DEFAULT_FONT") {
        register_path(
            path.into(),
            &mut collection,
            &mut source_cache,
            &mut default_fonts,
            &mut chain_families,
        );
    }

    // SLINT_FONT_PATH: OS-PATH-style list of additional fonts. Entries may be `.ttf`
    // files or directories (scanned non-recursively); everything found is appended to
    // the fallback chain after the primary.
    if let Some(path_list) = std::env::var_os("SLINT_FONT_PATH") {
        for entry in std::env::split_paths(&path_list) {
            if entry.is_file() {
                register_path(
                    entry,
                    &mut collection,
                    &mut source_cache,
                    &mut default_fonts,
                    &mut chain_families,
                );
            } else if let Ok(dir) = std::fs::read_dir(&entry) {
                for file in dir.flatten() {
                    register_path(
                        file.path(),
                        &mut collection,
                        &mut source_cache,
                        &mut default_fonts,
                        &mut chain_families,
                    );
                }
            }
        }
    }

    if !chain_families.is_empty() {
        for generic_family in [
            fontique::GenericFamily::SansSerif,
            fontique::GenericFamily::SystemUi,
            fontique::GenericFamily::UiSansSerif,
        ] {
            collection.set_generic_families(generic_family, chain_families.iter().copied());
        }
    }

    Collection { inner: collection, source_cache, default_fonts: Arc::new(default_fonts) }
}

#[derive(Clone)]
pub struct Collection {
    pub inner: fontique::Collection,
    pub source_cache: fontique::SourceCache,
    pub default_fonts: Arc<Vec<(std::path::PathBuf, fontique::QueryFont)>>,
}

impl Collection {
    pub fn query<'a>(&'a mut self) -> fontique::Query<'a> {
        self.inner.query(&mut self.source_cache)
    }

    pub fn get_font_for_info(
        &mut self,
        family_id: fontique::FamilyId,
        info: &fontique::FontInfo,
    ) -> Option<fontique::QueryFont> {
        get_font_for_info(&mut self.inner, &mut self.source_cache, family_id, info)
    }
}

fn get_font_for_info(
    collection: &mut fontique::Collection,
    source_cache: &mut fontique::SourceCache,
    family_id: fontique::FamilyId,
    info: &fontique::FontInfo,
) -> Option<fontique::QueryFont> {
    let mut query = collection.query(source_cache);
    query.set_families(std::iter::once(fontique::QueryFamily::from(family_id)));
    query.set_attributes(fontique::Attributes {
        weight: info.weight(),
        style: info.style(),
        width: info.width(),
    });
    let mut font = None;
    query.matches_with(|queried_font| {
        font = Some(queried_font.clone());
        fontique::QueryStatus::Stop
    });
    font
}

impl std::ops::Deref for Collection {
    type Target = fontique::Collection;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl std::ops::DerefMut for Collection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

pub const FALLBACK_FAMILIES: [fontique::GenericFamily; 2] =
    [fontique::GenericFamily::SystemUi, fontique::GenericFamily::SansSerif];

/// Wrapper around fontique::Blob to permit use of the blob as a key in the cache in the different renderers,
/// to map the blob to the native type face representation (skia_safe::Typeface, femtovg::FontId, QRawFont, etc.).
/// The use as key also ensures the blob remains strongly referenced, so that it doesn't vanish from the
/// shared SourceCache (parley prunes it).
#[derive(Clone)]
pub struct HashedBlob(fontique::Blob<u8>);
impl core::hash::Hash for HashedBlob {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.0.id().hash(state);
    }
}

impl PartialEq for HashedBlob {
    fn eq(&self, other: &Self) -> bool {
        self.0.id() == other.0.id()
    }
}

impl Eq for HashedBlob {}

impl From<fontique::Blob<u8>> for HashedBlob {
    fn from(value: fontique::Blob<u8>) -> Self {
        Self(value)
    }
}

impl AsRef<fontique::Blob<u8>> for HashedBlob {
    fn as_ref(&self) -> &fontique::Blob<u8> {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    // cspell:ignore fonttools varLib instancer opsz pyftsubset unicodes
    use skrifa::MetadataProvider;

    // Keep the embedded font small. Regenerate it with:
    //   fonttools varLib.instancer -o pinned.ttf Inter-VariableFont.ttf opsz=14
    //   pyftsubset pinned.ttf --unicodes="U+0000-DFFF,U+F900-10FFFF" --output-file=Inter-VariableFont.ttf
    #[test]
    fn embedded_fallback_font_is_minimal() {
        let data = include_bytes!("sharedfontique/Inter-VariableFont.ttf");
        let font = skrifa::FontRef::new(data).unwrap();

        let has_pua = font
            .charmap()
            .mappings()
            .any(|(cp, _)| matches!(cp, 0xE000..=0xF8FF | 0xF0000..=0xFFFFD | 0x100000..=0x10FFFD));
        assert!(!has_pua, "the embedded font maps Private Use Area codepoints; regenerate it");

        assert!(
            font.axes().iter().all(|axis| axis.tag() != "opsz"),
            "the embedded font still has an optical-size axis; pin it"
        );
    }

    #[test]
    #[cfg(not(any(target_family = "wasm", target_os = "nto")))]
    fn system_font_discovery() {
        use super::{create_collection, fontique};
        use std::process::Command;

        let font_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("sharedfontique/Inter-VariableFont.ttf");

        let Ok(case) = std::env::var("SLINT_TEST_FONT_COLLECTION") else {
            for (value, case) in [
                (None, "system"),
                (Some(""), "system"),
                (Some("0"), "system"),
                (Some("true"), "system"),
                (Some(" 1"), "system"),
                (Some("1"), "empty"),
                (Some("1"), "default"),
                (Some("1"), "path"),
                (Some("1"), "directory"),
            ] {
                let mut command = Command::new(std::env::current_exe().unwrap());
                command
                    .args(["--exact", "sharedfontique::tests::system_font_discovery"])
                    .env("SLINT_TEST_FONT_COLLECTION", case)
                    .env_remove("SLINT_NO_SYSTEM_FONTS")
                    .env_remove("SLINT_DEFAULT_FONT")
                    .env_remove("SLINT_FONT_PATH");
                if let Some(value) = value {
                    command.env("SLINT_NO_SYSTEM_FONTS", value);
                }
                match case {
                    "default" => {
                        command.env("SLINT_DEFAULT_FONT", &font_path);
                    }
                    "path" => {
                        command.env("SLINT_FONT_PATH", &font_path);
                    }
                    "directory" => {
                        command.env("SLINT_FONT_PATH", font_path.parent().unwrap());
                    }
                    _ => {}
                }
                assert!(command.status().unwrap().success(), "{value:?}, {case}");
            }
            return;
        };

        let data = include_bytes!("sharedfontique/Inter-VariableFont.ttf");
        for shared in [false, true] {
            let mut collection = create_collection(shared);
            let mut names: Vec<_> = collection.family_names().map(str::to_owned).collect();
            names.sort();
            match case.as_str() {
                "system" => {
                    let mut system = fontique::Collection::new(fontique::CollectionOptions {
                        shared,
                        ..Default::default()
                    });
                    let mut expected: Vec<_> = system.family_names().map(str::to_owned).collect();
                    expected.sort();
                    assert_eq!(names, expected);
                }
                "empty" => assert!(names.is_empty()),
                "default" | "path" | "directory" => {
                    assert_eq!(names, ["Inter"]);
                    assert_eq!(collection.default_fonts.len(), 1);
                    assert_eq!(collection.default_fonts[0].0, font_path);
                    let mut query = collection.query();
                    query.set_families([fontique::QueryFamily::Generic(
                        fontique::GenericFamily::SansSerif,
                    )]);
                    let mut matched = false;
                    query.matches_with(|font| {
                        assert_eq!(font.blob.data(), data.as_slice());
                        matched = true;
                        fontique::QueryStatus::Stop
                    });
                    assert!(matched);
                }
                _ => panic!("unknown test case: {case}"),
            }

            let registered = collection.register_fonts(data.to_vec().into(), None);
            let (family, infos) = registered.first().unwrap();
            assert_eq!(collection.family_name(*family), Some("Inter"));
            let font = collection.get_font_for_info(*family, &infos[0]).unwrap();
            assert_eq!(font.blob.data(), data.as_slice());
        }
    }
}
