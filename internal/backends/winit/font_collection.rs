// Copyright © SixtyFPS GmbH <info@slint.dev>
// SPDX-License-Identifier: GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0

use i_slint_common::sharedfontique::fontique;
use std::cell::Cell;
use std::thread::JoinHandle;

pub(crate) struct PendingFontCollection {
    worker: Cell<Option<JoinHandle<fontique::Collection>>>,
}

impl PendingFontCollection {
    pub fn new() -> Self {
        let worker = std::thread::Builder::new()
            .name("slint-font-collection".into())
            .spawn(|| {
                let create = || {
                    fontique::Collection::new(fontique::CollectionOptions {
                        shared: true,
                        system_fonts: true,
                    })
                };
                // Fontique's CoreText backend also calls Foundation, which needs a pool on worker threads.
                #[cfg(target_os = "macos")]
                return objc2::rc::autoreleasepool(|_| create());
                #[cfg(not(target_os = "macos"))]
                create()
            })
            .ok();
        Self { worker: Cell::new(worker) }
    }

    pub fn take(&self) -> Option<fontique::Collection> {
        self.worker
            .take()
            .map(|worker| worker.join().unwrap_or_else(|error| std::panic::resume_unwind(error)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    const INTER: &[u8] = include_bytes!("../../common/sharedfontique/Inter-VariableFont.ttf");

    fn collection_with_font(
        blob: fontique::Blob<u8>,
    ) -> (fontique::Collection, fontique::FamilyId) {
        let mut collection = fontique::Collection::new(fontique::CollectionOptions {
            shared: true,
            system_fonts: false,
        });
        let fonts = collection.register_fonts(blob, None);
        assert_eq!(fonts.len(), 1);
        let family = fonts[0].0;
        collection.set_generic_families(fontique::GenericFamily::SystemUi, [family].into_iter());
        (collection, family)
    }

    #[test]
    fn takes_the_running_workers_collection_once() {
        let blob = fontique::Blob::new(Arc::new(INTER));
        let blob_id = blob.id();
        let (ready, started) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let (collection, family) = collection_with_font(blob);
            ready.send(family).unwrap();
            wait.recv().unwrap();
            collection
        });
        let family = started.recv().unwrap();
        assert!(!worker.is_finished());
        let pending = PendingFontCollection { worker: Cell::new(Some(worker)) };
        let consumer = std::thread::spawn(move || {
            let collection = pending.take().unwrap();
            assert!(pending.take().is_none());
            collection
        });
        release.send(()).unwrap();
        let mut collection = consumer.join().unwrap();
        assert_eq!(collection.family_id("Inter"), Some(family));
        let mut source_cache = fontique::SourceCache::new_shared();
        let mut query = collection.query(&mut source_cache);
        query.set_families([fontique::QueryFamily::Generic(fontique::GenericFamily::SystemUi)]);
        let mut matched = None;
        query.matches_with(|font| {
            matched = Some((font.family.0, font.blob.id()));
            fontique::QueryStatus::Stop
        });
        assert_eq!(matched, Some((family, blob_id)));
    }

    #[test]
    fn dropping_pending_does_not_wait_or_retain_the_collection() {
        struct FontData(mpsc::Sender<()>);
        impl AsRef<[u8]> for FontData {
            fn as_ref(&self) -> &[u8] {
                INTER
            }
        }
        impl Drop for FontData {
            fn drop(&mut self) {
                let _ = self.0.send(());
            }
        }

        let (font_dropped, font_drop) = mpsc::channel();
        let (ready, started) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let (collection, _) =
                collection_with_font(fontique::Blob::new(Arc::new(FontData(font_dropped))));
            ready.send(()).unwrap();
            wait.recv().unwrap();
            collection
        });
        started.recv().unwrap();
        let pending = PendingFontCollection { worker: Cell::new(Some(worker)) };
        let (dropped, drop_completed) = mpsc::channel();
        let dropping = std::thread::spawn(move || {
            drop(pending);
            dropped.send(()).unwrap();
        });
        let dropped_without_waiting = drop_completed.recv_timeout(Duration::from_secs(5));
        release.send(()).unwrap();
        dropping.join().unwrap();
        assert!(dropped_without_waiting.is_ok());
        font_drop.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(PendingFontCollection { worker: Cell::new(None) }.take().is_none());
    }

    #[test]
    fn worker_panic_preserves_its_payload() {
        let payload = Arc::new(());
        let worker_payload = payload.clone();
        let pending = PendingFontCollection {
            worker: Cell::new(Some(std::thread::spawn(move || {
                std::panic::panic_any(worker_payload)
            }))),
        };
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pending.take()));
        let Err(error) = panic else { panic!("the worker panic was swallowed") };
        assert!(Arc::ptr_eq(error.downcast_ref::<Arc<()>>().unwrap(), &payload));
        assert!(pending.take().is_none());
    }

    #[test]
    fn no_worker_leaves_collection_creation_to_the_context() {
        let pending = PendingFontCollection { worker: Cell::new(None) };
        assert!(pending.take().is_none());
        assert!(pending.take().is_none());
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn font_paths_are_loaded_after_worker_handoff() {
        const CHILD_DIRECTORY: &str = "SLINT_TEST_FONT_COLLECTION_DIRECTORY";
        if let Some(directory) = std::env::var_os(CHILD_DIRECTORY) {
            let directory = std::path::PathBuf::from(directory);
            let primary = directory.join("primary.ttf");
            let fallback = directory.join("fallback.ttf");
            let pending = PendingFontCollection::new();
            let mut raw_collection = pending.take().expect("font discovery worker failed to spawn");
            let system_families = raw_collection
                .generic_families(fontique::GenericFamily::SystemUi)
                .collect::<Vec<_>>();
            std::fs::write(&primary, INTER).unwrap();
            std::fs::write(
                &fallback,
                include_bytes!("../../../tests/screenshots/fonts/NotoSansSymbols2-Regular.ttf"),
            )
            .unwrap();
            let mut collection =
                i_slint_common::sharedfontique::init_collection(raw_collection, true);
            assert_eq!(
                collection.default_fonts.iter().map(|(path, _)| path).collect::<Vec<_>>(),
                [&primary, &fallback]
            );
            let families = collection
                .default_fonts
                .iter()
                .map(|(_, font)| font.family.0)
                .chain(system_families)
                .collect::<Vec<_>>();
            assert_eq!(
                collection.generic_families(fontique::GenericFamily::SystemUi).collect::<Vec<_>>(),
                families
            );
            return;
        }

        let directory = std::env::temp_dir().join(format!(
            "slint-font-collection-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let primary = directory.join("primary.ttf");
        let fallback = directory.join("fallback.ttf");
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "font_collection::tests::font_paths_are_loaded_after_worker_handoff"])
            .env(CHILD_DIRECTORY, &directory)
            .env("SLINT_DEFAULT_FONT", &primary)
            .env("SLINT_FONT_PATH", std::env::join_paths([&primary, &fallback, &fallback]).unwrap())
            .output();
        let child_created_fonts = primary.is_file() && fallback.is_file();
        std::fs::remove_dir_all(&directory).unwrap();
        let output = output.unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(child_created_fonts, "the child test did not run");
    }
}
