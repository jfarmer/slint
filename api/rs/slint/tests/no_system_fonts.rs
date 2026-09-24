// Copyright © SixtyFPS GmbH <info@slint.dev>
// SPDX-License-Identifier: GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0

#![cfg(all(feature = "std", feature = "renderer-software", not(target_family = "wasm")))]

use slint::ComponentHandle;
use slint::platform::WindowAdapter;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use std::rc::Rc;

slint::slint! {
    import "../../../../tests/screenshots/fonts/NotoSans-Regular.ttf";
    export component FontTest inherits Window {
        width: 200px;
        height: 40px;
        background: white;
        in property <string> family: "Noto Sans";
        Text {
            text: "Supplied fonts";
            font-family: root.family;
            font-size: 24px;
            color: black;
        }
    }
}

struct TestPlatform(Rc<MinimalSoftwareWindow>);

impl slint::platform::Platform for TestPlatform {
    fn create_window_adapter(
        &self,
    ) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

fn font_families() -> Vec<String> {
    i_slint_core::with_global_context(
        || unreachable!(),
        |ctx| {
            ctx.font_context().borrow_mut().collection.family_names().map(str::to_owned).collect()
        },
    )
    .unwrap()
}

#[test]
fn supplied_fonts_render_without_system_fonts() {
    let Ok(registration) = std::env::var("SLINT_TEST_FONT_REGISTRATION") else {
        for registration in ["memory", "path"] {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "supplied_fonts_render_without_system_fonts"])
                .env("SLINT_TEST_FONT_REGISTRATION", registration)
                .env("SLINT_NO_SYSTEM_FONTS", "1")
                .env_remove("SLINT_DEFAULT_FONT")
                .env_remove("SLINT_FONT_PATH")
                .status()
                .unwrap();
            assert!(status.success(), "{registration}");
        }
        return;
    };

    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(TestPlatform(window.clone()))).unwrap();
    #[cfg(not(target_os = "nto"))]
    assert!(font_families().is_empty());

    let ui = FontTest::new().unwrap();
    assert!(font_families().iter().any(|name| name == "Noto Sans"));
    ui.show().unwrap();
    window.set_size(slint::PhysicalSize::new(200, 40));
    let render = || {
        let mut pixels = vec![slint::Rgb8Pixel::new(255, 255, 255); 200 * 40];
        window.request_redraw();
        assert!(window.draw_if_needed(|renderer| {
            renderer.render(&mut pixels, 200);
        }));
        assert!(pixels.iter().any(|p| p.r < 128 && p.g < 128 && p.b < 128));
        pixels
    };
    let imported = render();

    match registration.as_str() {
        "memory" => window
            .renderer()
            .register_font_from_memory(include_bytes!(
                "../../../../internal/common/sharedfontique/Inter-VariableFont.ttf"
            ))
            .unwrap(),
        "path" => window
            .renderer()
            .register_font_from_path(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../../internal/common/sharedfontique/Inter-VariableFont.ttf")
                    .as_path(),
            )
            .unwrap(),
        _ => unreachable!(),
    }
    ui.set_family("Inter".into());
    assert!(font_families().iter().any(|name| name == "Inter"));
    assert_ne!(imported, render());
}
