// Copyright © SixtyFPS GmbH <info@slint.dev>
// SPDX-License-Identifier: GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0

//! Keeps a freshly shown window invisible until its first frame is submitted.
//!
//! wgpu's Metal surface hands out no drawable while the hosting `NSWindow` reports itself
//! as not visible (gfx-rs/wgpu#8309).
//! A window that hasn't been ordered front yet counts as such.
//! So the frame rendered before showing the window is dropped,
//! and AppKit maps the window with an empty content layer.
//! The title bar shows up alone until `occlusionState` catches up and a redraw lands.
//! Ordering the window front with an alpha of zero hides that gap.
//! The window server still reports such a window as visible, so the redraw goes through,
//! and the alpha is restored once a frame is submitted.
//!
//! wgpu fixes this in <https://github.com/gfx-rs/wgpu/pull/10302>.
//! Remove this module once every wgpu version Slint builds against contains that fix.

use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2_app_kit::{NSView, NSWindow, NSWindowOcclusionState};
use objc2_quartz_core::{CAMetalLayer, CATransaction};

/// Makes a window fully transparent for as long as this value lives.
/// Drop it when the first frame is submitted.
pub(crate) struct RevealOnFirstFrame {
    ns_window: Retained<NSWindow>,
    alpha: f64,
    /// With `SLINT_REVEAL_DELAY_MS`, when the hold after the first submitted frame ends.
    hold_until: Option<Instant>,
    /// With `SLINT_PRESENT_WITH_TRANSACTION`, the Metal layer that presents inside a
    /// Core Animation transaction until the window is revealed.
    transaction_layer: Option<Retained<CAMetalLayer>>,
    /// `SLINT_PRESENT_WITH_TRANSACTION=reveal-first` restores the alpha inside the
    /// transaction before the frame is rendered instead of after it is submitted.
    reveal_first: bool,
}

impl RevealOnFirstFrame {
    pub(crate) fn new(winit_window: &winit::window::Window) -> Option<Self> {
        let ns_view = ns_view(winit_window)?;
        let ns_window = ns_view.window()?;
        let alpha = ns_window.alphaValue();
        let _span = i_slint_common::startup_trace::Span::new("appkit.hide_initial_frame");
        ns_window.setAlphaValue(0.0);
        let mode = std::env::var("SLINT_PRESENT_WITH_TRANSACTION").ok();
        let reveal_first = mode.as_deref() == Some("reveal-first");
        let transaction_layer =
            (mode.as_deref() == Some("1") || reveal_first).then(|| metal_layer(ns_view)).flatten();
        if let Some(layer) = &transaction_layer {
            layer.setPresentsWithTransaction(true);
        }
        Some(Self { ns_window, alpha, hold_until: None, transaction_layer, reveal_first })
    }

    /// With `SLINT_PRESENT_WITH_TRANSACTION`, opens a Core Animation transaction that
    /// commits the frame's drawable together with the reveal. Drop it after the reveal.
    pub(crate) fn frame_transaction(&self) -> Option<FrameTransaction> {
        self.transaction_layer.as_ref()?;
        CATransaction::begin();
        // Until AppKit reports the window visible, wgpu hands out no drawable to reveal with.
        if self.reveal_first
            && self.ns_window.occlusionState().contains(NSWindowOcclusionState::Visible)
        {
            let _span = i_slint_common::startup_trace::Span::new("appkit.reveal_first_frame");
            self.ns_window.setAlphaValue(self.alpha);
        }
        Some(FrameTransaction)
    }

    /// Whether the window stays transparent after a submitted frame.
    /// `SLINT_REVEAL_DELAY_MS=<n>` keeps it transparent for n ms after the first submitted frame.
    pub(crate) fn hold_after_submit(&mut self) -> bool {
        let Some(delay) = reveal_delay() else { return false };
        let now = Instant::now();
        now < *self.hold_until.get_or_insert(now + delay)
    }
}

/// Commits the Core Animation transaction opened by [`RevealOnFirstFrame::frame_transaction`].
pub(crate) struct FrameTransaction;

impl Drop for FrameTransaction {
    fn drop(&mut self) {
        let _span = i_slint_common::startup_trace::Span::new("appkit.commit_frame_transaction");
        CATransaction::commit();
    }
}

/// The `CAMetalLayer` wgpu renders into: the view's layer or one of its sublayers.
fn metal_layer(ns_view: &NSView) -> Option<Retained<CAMetalLayer>> {
    let layer = ns_view.layer()?;
    let sublayers = unsafe { layer.sublayers() }.map(|sublayers| sublayers.to_vec());
    core::iter::once(layer)
        .chain(sublayers.into_iter().flatten())
        .find_map(|layer| layer.downcast::<CAMetalLayer>().ok())
}

fn reveal_delay() -> Option<Duration> {
    std::env::var("SLINT_REVEAL_DELAY_MS").ok()?.parse().ok().map(Duration::from_millis)
}

impl Drop for RevealOnFirstFrame {
    fn drop(&mut self) {
        let _span = i_slint_common::startup_trace::Span::new("appkit.reveal_first_frame");
        self.ns_window.setAlphaValue(self.alpha);
        if let Some(layer) = &self.transaction_layer {
            layer.setPresentsWithTransaction(false);
        }
    }
}

pub(crate) fn ns_view(winit_window: &winit::window::Window) -> Option<&NSView> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let RawWindowHandle::AppKit(handle) = winit_window.window_handle().ok()?.as_raw() else {
        return None;
    };
    Some(unsafe { handle.ns_view.cast().as_ref() })
}
