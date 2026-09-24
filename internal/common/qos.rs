// Copyright © SixtyFPS GmbH <info@slint.dev>
// SPDX-License-Identifier: GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0

//! Quality-of-service classes of macOS threads. A spawned thread starts at the default class,
//! below the main thread of an application.

#![allow(unsafe_code)]

/// The calling thread's QoS class as its raw `qos_class_t` value.
pub fn current() -> u32 {
    let mut class = 0u32;
    let mut relative_priority: libc::c_int = 0;
    // SAFETY: `qos_class_t` is a `repr(u32)` enum, so `class` has its layout. The call only
    // writes the calling thread's class and relative priority into the out-parameters.
    let result = unsafe {
        libc::pthread_get_qos_class_np(
            libc::pthread_self(),
            (&mut class as *mut u32).cast(),
            &mut relative_priority,
        )
    };
    if result == 0 { class } else { 0 }
}

/// Raises the calling thread to `class` when it runs below it.
pub fn raise_self_to(class: u32) {
    use libc::qos_class_t::*;
    let Some(target) = [
        QOS_CLASS_USER_INTERACTIVE,
        QOS_CLASS_USER_INITIATED,
        QOS_CLASS_DEFAULT,
        QOS_CLASS_UTILITY,
    ]
    .into_iter()
    .find(|known| *known as u32 == class) else {
        return;
    };
    if current() < class {
        // SAFETY: changes only the calling thread's scheduling class; failure leaves it
        // unchanged.
        unsafe { libc::pthread_set_qos_class_self_np(target, 0) };
    }
}
