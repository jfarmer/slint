// Copyright © SixtyFPS GmbH <info@slint.dev>
// SPDX-License-Identifier: GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0

#![cfg(all(
    feature = "std",
    feature = "shared-parley",
    any(target_os = "macos", target_os = "windows", target_os = "linux", target_os = "freebsd")
))]

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::{Arc, Barrier};

use i_slint_common::sharedfontique::{self, fontique};
use i_slint_core::graphics::FontRequest;
use i_slint_core::platform::{Platform, PlatformError, WindowAdapter};
use i_slint_core::{InternalToken, SlintContext, SlintContextWeak};

const FONT: &[u8] = include_bytes!("../../common/sharedfontique/Inter-VariableFont.ttf");
const REGISTERED_FONT: &[u8] =
    include_bytes!("../../../tests/screenshots/fonts/NotoSans-Regular.ttf");

struct PreparedPlatform {
    collection: RefCell<Option<fontique::Collection>>,
    on_bind: Option<fn(&SlintContext)>,
}

impl Platform for PreparedPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        unreachable!()
    }

    fn take_font_collection(&self, _: InternalToken) -> Option<fontique::Collection> {
        self.collection.borrow_mut().take()
    }

    fn bind_context(&self, context: SlintContextWeak, _: InternalToken) {
        if let Some(on_bind) = self.on_bind {
            on_bind(&context.upgrade().unwrap());
        }
    }
}

struct UnpreparedPlatform;

impl Platform for UnpreparedPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        unreachable!()
    }
}

fn latin_fallback() -> fontique::FallbackKey {
    fontique::FallbackKey::new(fontique::Script::from_bytes(*b"Latn"), None)
}

fn prepare_fixture(name: &'static str) -> (fontique::Collection, fontique::FamilyId) {
    std::thread::spawn(move || {
        let mut collection = fontique::Collection::new(fontique::CollectionOptions {
            shared: true,
            system_fonts: false,
        });
        let fonts = collection.register_fonts(
            fontique::Blob::new(Arc::new(FONT)),
            Some(fontique::FontInfoOverride { family_name: Some(name), ..Default::default() }),
        );
        let family = fonts[0].0;
        assert!(collection.set_fallbacks(latin_fallback(), [family].into_iter()));
        (collection, family)
    })
    .join()
    .unwrap()
}

fn prepared_context(collection: fontique::Collection) -> SlintContext {
    SlintContext::new(Box::new(PreparedPlatform {
        collection: RefCell::new(Some(collection)),
        on_bind: None,
    }))
}

fn query_font(
    context: &SlintContext,
    family: fontique::QueryFamily<'_>,
    fallback: Option<fontique::FallbackKey>,
    character: char,
) -> Option<fontique::QueryFont> {
    let mut context = context.font_context().borrow_mut();
    let context = &mut context.inner;
    let mut query = context.collection.query(&mut context.source_cache);
    query.set_families([family]);
    if let Some(fallback) = fallback {
        query.set_fallbacks(fallback);
    }
    let mut result = None;
    query.matches_with(|font| {
        if font.charmap().and_then(|charmap| charmap.map(character)).is_some() {
            result = Some(font.clone());
            fontique::QueryStatus::Stop
        } else {
            fontique::QueryStatus::Continue
        }
    });
    result
}

#[test]
fn prepared_collection_preserves_fonts_and_fallbacks() {
    let (collection, family) = prepare_fixture("Slint Prepared Font");
    let context = SlintContext::new(Box::new(PreparedPlatform {
        collection: RefCell::new(Some(collection)),
        on_bind: Some(|context| {
            let font = query_font(context, "Slint Prepared Font".into(), None, 'A').unwrap();
            assert_eq!(font.blob.as_ref(), FONT);
            context.font_context().borrow_mut().register_static_font(REGISTERED_FONT);
        }),
    }));

    let named = query_font(&context, "Slint Prepared Font".into(), None, 'A').unwrap();
    assert_eq!(named.family.0, family);
    assert_eq!(named.blob.as_ref(), FONT);

    let fallback =
        query_font(&context, "Slint Missing Font".into(), Some(latin_fallback()), 'A').unwrap();
    assert_eq!(fallback.family, named.family);
    assert_eq!(fallback.blob.id(), named.blob.id());
    assert!(context.platform().take_font_collection(InternalToken).is_none());
    let registered = query_font(&context, "Noto Sans".into(), None, 'A').unwrap();
    assert_eq!(registered.blob.as_ref(), REGISTERED_FONT);
}

#[test]
fn repeated_contexts_keep_separate_runtime_registrations() {
    let (first_collection, first_family) = prepare_fixture("Slint First Font");
    let first = prepared_context(first_collection);
    let before_registration =
        font_signature(&first, query_font(&first, "Noto Sans".into(), None, 'A'));
    first.font_context().borrow_mut().register_static_font(REGISTERED_FONT);

    let (second_collection, second_family) = prepare_fixture("Slint Second Font");
    let second = prepared_context(second_collection);
    assert!(query_font(&first, "Slint Second Font".into(), None, 'A').is_none());
    assert!(query_font(&second, "Slint First Font".into(), None, 'A').is_none());
    assert_eq!(
        font_signature(&second, query_font(&second, "Noto Sans".into(), None, 'A')),
        before_registration
    );

    let registered = query_font(&first, "Noto Sans".into(), None, 'A').unwrap();
    assert_eq!(registered.blob.as_ref(), REGISTERED_FONT);
    for (context, family) in [(&first, first_family), (&second, second_family)] {
        let fallback =
            query_font(context, "Slint Missing Font".into(), Some(latin_fallback()), 'A').unwrap();
        assert_eq!(fallback.family.0, family);
    }
}

#[test]
fn concurrent_contexts_keep_their_own_prepared_collection() {
    let barrier = Arc::new(Barrier::new(2));
    let workers = ["Slint Concurrent First", "Slint Concurrent Second"].map(|name| {
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            let (collection, family) = prepare_fixture(name);
            barrier.wait();
            let context = prepared_context(collection);
            let font = query_font(&context, name.into(), None, 'A').unwrap();
            assert_eq!(font.family.0, family);
            assert_eq!(font.blob.as_ref(), FONT);
            let other = if name == "Slint Concurrent First" {
                "Slint Concurrent Second"
            } else {
                "Slint Concurrent First"
            };
            assert!(query_font(&context, other.into(), None, 'A').is_none());
        })
    });
    for worker in workers {
        worker.join().unwrap();
    }
}

fn font_signature(
    context: &SlintContext,
    font: Option<fontique::QueryFont>,
) -> Option<(String, u32, u64)> {
    font.map(|font| {
        let family = context
            .font_context()
            .borrow_mut()
            .collection
            .family_name(font.family.0)
            .unwrap()
            .to_owned();
        let mut hash = DefaultHasher::new();
        font.blob.as_ref().hash(&mut hash);
        (family, font.index, hash.finish())
    })
}

#[test]
#[cfg_attr(miri, ignore)]
fn preparation_preserves_system_selection_and_unprepared_initialization() {
    with_autorelease_pool(compare_system_collections);
}

fn with_autorelease_pool<R>(f: fn() -> R) -> R {
    #[cfg(target_os = "macos")]
    return objc2::rc::autoreleasepool(|_| f());
    #[cfg(not(target_os = "macos"))]
    f()
}

fn compare_system_collections() {
    let worker = std::thread::spawn(|| {
        with_autorelease_pool(|| {
            fontique::Collection::new(fontique::CollectionOptions {
                shared: true,
                system_fonts: true,
            })
        })
    });
    let unprepared = SlintContext::new(Box::new(UnpreparedPlatform));
    let prepared = prepared_context(worker.join().unwrap());
    let families = |context: &SlintContext| {
        let mut names: Vec<_> = context
            .font_context()
            .borrow_mut()
            .collection
            .family_names()
            .map(str::to_owned)
            .collect();
        names.sort();
        names
    };
    assert_eq!(families(&prepared), families(&unprepared));

    let default_font = |context: &SlintContext| {
        let mut context = context.font_context().borrow_mut();
        let context = &mut context.inner;
        FontRequest::default().query_fontique(&mut context.collection, &mut context.source_cache)
    };
    assert_eq!(
        font_signature(&prepared, default_font(&prepared)),
        font_signature(&unprepared, default_font(&unprepared))
    );

    for (script, character) in [("Latn", 'A'), ("Arab", 'ا'), ("Hani", '中')] {
        let key = fontique::FallbackKey::new(script.parse::<fontique::Script>().unwrap(), None);
        for family in [
            fontique::QueryFamily::Named("Slint Missing Font"),
            fontique::QueryFamily::Generic(sharedfontique::FALLBACK_FAMILIES[0]),
        ] {
            assert_eq!(
                font_signature(&prepared, query_font(&prepared, family, Some(key), character)),
                font_signature(&unprepared, query_font(&unprepared, family, Some(key), character)),
                "fallback differs for {script} and {family:?}"
            );
        }
    }
}
