//! Audio ↔ GUI shared state. The audio thread writes; the GUI thread reads.
//! Everything here is either an atomic or a short-lived `try_lock`-only
//! mutex, so the audio thread never blocks on the GUI.

use std::sync::atomic::{AtomicU32, AtomicU8, AtomicUsize, Ordering};

pub const SCOPE_SIZE: usize = 2048;

pub struct SharedState {
    /// Output scope ring buffer (mono mix of L+R), f32 bit-cast into u32.
    pub scope: Vec<AtomicU32>,
    pub scope_write: AtomicUsize,

    /// Current *effective* read delay per channel in milliseconds — the
    /// smoothed/slewed value including wow & flutter modulation, so the
    /// TapeView's tap markers drift and swoop exactly like the audio does.
    pub delay_l_ms: AtomicU32,
    pub delay_r_ms: AtomicU32,

    /// Engine's current sample rate, published by `Engine::init`.
    pub sample_rate: AtomicU32,

    /// Active visual theme as a u8: 0 = Classic, 1 = Terminal. Read by the
    /// custom widgets (`TapeView`, `ScopeView`) to swap accent colors at
    /// draw time. The audio thread doesn't touch this — it lives on
    /// `SharedState` purely as a low-friction way for those widgets to
    /// observe the current theme without plumbing a lens through their
    /// `&self` draw methods.
    pub theme: AtomicU8,
}

impl SharedState {
    pub fn new() -> Self {
        let mut scope = Vec::with_capacity(SCOPE_SIZE);
        for _ in 0..SCOPE_SIZE {
            scope.push(AtomicU32::new(0));
        }
        Self {
            scope,
            scope_write: AtomicUsize::new(0),
            delay_l_ms: AtomicU32::new(0),
            delay_r_ms: AtomicU32::new(0),
            sample_rate: AtomicU32::new(48_000),
            theme: AtomicU8::new(0), // Classic by default
        }
    }

    pub fn scope_push(&self, sample: f32) {
        let pos = self.scope_write.fetch_add(1, Ordering::Relaxed) % SCOPE_SIZE;
        self.scope[pos].store(sample.to_bits(), Ordering::Relaxed);
    }

    pub fn scope_load_at(&self, idx: usize) -> f32 {
        f32::from_bits(self.scope[idx].load(Ordering::Relaxed))
    }

    pub fn scope_write_pos(&self) -> usize {
        self.scope_write.load(Ordering::Relaxed)
    }

    pub fn store_delay_ms(&self, l: f32, r: f32) {
        self.delay_l_ms.store(l.to_bits(), Ordering::Relaxed);
        self.delay_r_ms.store(r.to_bits(), Ordering::Relaxed);
    }

    pub fn load_delay_ms(&self) -> (f32, f32) {
        (
            f32::from_bits(self.delay_l_ms.load(Ordering::Relaxed)),
            f32::from_bits(self.delay_r_ms.load(Ordering::Relaxed)),
        )
    }
}

impl Default for SharedState {
    fn default() -> Self {
        Self::new()
    }
}
