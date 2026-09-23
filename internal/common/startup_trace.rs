// Copyright © SixtyFPS GmbH <info@slint.dev>
// SPDX-License-Identifier: GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0

#[cfg(feature = "shared-fontique")]
mod implementation {
    use std::cell::RefCell;
    use std::sync::OnceLock;
    use std::time::Instant;

    static ORIGIN: OnceLock<Option<Instant>> = OnceLock::new();

    struct Sample {
        name: &'static str,
        start_ns: u64,
        duration_ns: u64,
        amount: u64,
    }

    std::thread_local! {
        static SAMPLES: RefCell<Vec<Sample>> = const { RefCell::new(Vec::new()) };
    }

    fn origin() -> Option<Instant> {
        *ORIGIN.get_or_init(|| {
            (std::env::var("SLINT_STARTUP_DIAGNOSTICS").as_deref() == Ok("1")).then(Instant::now)
        })
    }

    pub fn enabled() -> bool {
        origin().is_some()
    }

    pub struct Span {
        name: &'static str,
        start: Option<Instant>,
        amount: u64,
    }

    impl Span {
        pub fn new(name: &'static str) -> Self {
            Self { name, start: enabled().then(Instant::now), amount: 1 }
        }

        pub fn with_amount(mut self, amount: u64) -> Self {
            self.amount = amount;
            self
        }
    }

    impl Drop for Span {
        fn drop(&mut self) {
            if let Some(start) = self.start {
                let duration_ns = start.elapsed().as_nanos() as u64;
                let start_ns = start.duration_since(origin().unwrap()).as_nanos() as u64;
                SAMPLES.with_borrow_mut(|samples| {
                    samples.push(Sample {
                        name: self.name,
                        start_ns,
                        duration_ns,
                        amount: self.amount,
                    })
                });
            }
        }
    }

    pub fn record(name: &'static str, amount: u64) {
        if let Some(origin) = origin() {
            let start_ns = origin.elapsed().as_nanos() as u64;
            SAMPLES.with_borrow_mut(|samples| {
                samples.push(Sample { name, start_ns, duration_ns: 0, amount })
            });
        }
    }

    // Call on the instrumented UI thread after the responsiveness probe completes.
    // Buffered spans include nested wall times and must not be summed indiscriminately.
    pub fn flush() {
        if !enabled() {
            return;
        }
        use std::io::Write;
        SAMPLES.with_borrow_mut(|samples| {
            let stderr = std::io::stderr();
            let mut output = std::io::BufWriter::new(stderr.lock());
            for sample in samples.drain(..) {
                let _ = writeln!(
                    output,
                    "slint-poc-span {} {} {} {}",
                    sample.name, sample.start_ns, sample.duration_ns, sample.amount
                );
            }
        });
    }
}

#[cfg(not(feature = "shared-fontique"))]
mod implementation {
    pub struct Span;
    impl Span {
        pub fn new(_: &'static str) -> Self {
            Self
        }
        pub fn with_amount(self, _: u64) -> Self {
            self
        }
    }
    pub fn enabled() -> bool {
        false
    }
    pub fn record(_: &'static str, _: u64) {}
    pub fn flush() {}
}

pub use implementation::{Span, enabled, flush, record};
