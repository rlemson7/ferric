//! Ferric DSP — a stereo tape delay modeled on the behavior of the Erica
//! Synths Black Stereo Delay:
//!
//! - **Tape** mode: delay-time changes slew the read head instead of jumping,
//!   producing the classic pitch swoop, plus a fixed gentle top-end rolloff
//!   in the repeats.
//! - **Digital** mode: time changes are stepped crossfades — clickless, but
//!   with the "musical digital artifacts" of a rack delay.
//! - **Ping-pong** mode: mono-summed input feeds the left line and feedback
//!   crosses L→R→L so repeats bounce between speakers.
//! - Feedback path: DC-safe highpass + tone lowpass + tape saturation +
//!   soft-limiting, so high feedback blooms into bounded self-oscillation
//!   instead of exploding.
//! - **Hold** freezes the loop at unity feedback with the coloration stage
//!   bypassed (clean repeats); **Add** overdubs live input into the held loop.
//! - **Reverse** plays the delay tail backwards in delay-length chunks with
//!   an equal-power crossfade at each chunk seam.
//! - Wow (slow, depth scales with delay time) & flutter (fast, sine + random)
//!   modulate the read head like a worn transport.

use crate::state::SharedState;
use crate::{DelayMode, LfoCfg, LfoShape, LfoTarget, ParamValues, SyncDivision};
use std::sync::Arc;

pub const MIN_DELAY_MS: f32 = 3.0;
pub const MAX_DELAY_MS: f32 = 3000.0;
/// Max stereo-spread skew applied to the right channel's delay time.
pub const MAX_SPREAD_SKEW: f32 = 0.35;
/// Reverse mode reads up to 2× the (spread-skewed) max delay behind the
/// write head, so the buffer is sized past that: 2 × 3 s × 1.35 = 8.1 s.
const BUFFER_SECONDS: f32 = 8.5;

/// Per-channel time/filter state. Kept as a plain struct (not methods on
/// `Engine`) so `process_sample` can hold `&mut` to one channel while
/// reading the shared sample buffer.
#[derive(Default, Clone, Copy)]
struct ChannelState {
    /// Current settled read delay in samples. Tape mode slews this toward
    /// the target; digital mode jumps it and crossfades from `fade_from`.
    cur_delay: f32,
    fade_from: f32,
    fade_pos: f32,
    fading: bool,
    /// Reverse-chunk phase [0, rev_d) and the chunk length latched at the
    /// start of each chunk (so a mid-chunk time change can't corrupt the
    /// ramp arithmetic).
    rev_phase: f32,
    rev_d: f32,
    /// Feedback-path one-pole states. The highpass is implemented as
    /// `x - lowpass(x)` so both are plain one-pole accumulators.
    lp: f32,
    hp_lp: f32,
    /// Smoothed random component of flutter.
    flutter_rand: f32,
    flutter_target: f32,
}

#[derive(Default, Clone, Copy)]
struct LfoState {
    /// Current LFO phase, [0, 1).
    phase: f32,
    /// Latched random value for S&H. Refreshed on every cycle wrap.
    sh_value: f32,
    /// Smooth-random "from" and "to" targets, interpolated across one cycle.
    rand_a: f32,
    rand_b: f32,
}

pub struct Engine {
    sample_rate: f32,
    buffer_samples: usize,

    buffer_l: Vec<f32>,
    buffer_r: Vec<f32>,
    write: usize,

    ch_l: ChannelState,
    ch_r: ChannelState,

    /// Snap `cur_delay` straight to the target on the next processed sample
    /// instead of slewing/fading from 0 — set by `init` and `reset` so a
    /// fresh instance doesn't open with a multi-second tape swoop.
    snap_next: bool,

    // Per-sample one-pole smoothed params (coefficient matches STASIS).
    feedback_s: f32,
    spread_s: f32,
    drive_s: f32,
    tone_s: f32,
    wow_s: f32,
    flutter_s: f32,
    mix_s: f32,

    wow_phase: f32,
    flutter_phase: f32,
    flutter_countdown: i32,

    rng: u32,

    lfo: [LfoState; 2],

    state: Arc<SharedState>,
}

impl Engine {
    #[cfg(test)]
    pub fn new() -> Self {
        Self::with_state(Arc::new(SharedState::new()))
    }

    pub fn with_state(state: Arc<SharedState>) -> Self {
        Self {
            sample_rate: 48000.0,
            buffer_samples: 0,
            buffer_l: Vec::new(),
            buffer_r: Vec::new(),
            write: 0,
            ch_l: ChannelState::default(),
            ch_r: ChannelState::default(),
            snap_next: true,
            feedback_s: 0.4,
            spread_s: 0.0,
            drive_s: 0.25,
            tone_s: 0.0,
            wow_s: 0.0,
            flutter_s: 0.0,
            mix_s: 0.5,
            wow_phase: 0.0,
            flutter_phase: 0.0,
            flutter_countdown: 0,
            rng: 0xDEAD_BEEF,
            lfo: [LfoState::default(); 2],
            state,
        }
    }

    pub fn init(&mut self, sample_rate: f32) {
        self.sample_rate = if sample_rate > 1.0 { sample_rate } else { 48000.0 };
        self.buffer_samples = (BUFFER_SECONDS * self.sample_rate) as usize;
        self.buffer_l = vec![0.0; self.buffer_samples];
        self.buffer_r = vec![0.0; self.buffer_samples];
        self.write = 0;
        self.state
            .sample_rate
            .store(self.sample_rate as u32, std::sync::atomic::Ordering::Relaxed);
        self.lfo = [LfoState::default(); 2];
        self.reset();
    }

    pub fn reset(&mut self) {
        // A host reset (bypass cycle, transport jump) should not leave a
        // stale delay tail ringing — zero the lines in place (no realloc).
        self.buffer_l.iter_mut().for_each(|s| *s = 0.0);
        self.buffer_r.iter_mut().for_each(|s| *s = 0.0);
        self.ch_l = ChannelState::default();
        self.ch_r = ChannelState::default();
        self.snap_next = true;
        self.wow_phase = 0.0;
        self.flutter_phase = 0.0;
        self.flutter_countdown = 0;
    }

    /// Advance both LFOs by one block and return their current outputs in
    /// [-1, 1]. Sync-divided LFOs lock to `pos_beats`; free-running LFOs
    /// step by `block_size / sample_rate * rate_hz`. If the host doesn't
    /// expose tempo or beat position (some standalone hosts), sync divisions
    /// fall back to free-running at `rate_hz`.
    pub fn step_lfos(
        &mut self,
        tempo_bpm: Option<f64>,
        pos_beats: Option<f64>,
        block_size: usize,
        cfgs: &[LfoCfg; 2],
    ) -> [f32; 2] {
        let mut out = [0.0_f32; 2];
        for i in 0..2 {
            let cfg = cfgs[i];

            let new_phase_raw = match (cfg.sync, tempo_bpm, pos_beats) {
                (SyncDivision::Free, _, _) | (_, None, _) | (_, _, None) => {
                    let inc = (cfg.rate_hz as f64 / self.sample_rate as f64)
                        * block_size as f64;
                    (self.lfo[i].phase as f64 + inc).rem_euclid(1.0) as f32
                }
                (sync, Some(_), Some(beats)) => {
                    if !beats.is_finite() {
                        // Host returned a bad pos_beats (transport jump etc.);
                        // hold the previous phase rather than going NaN.
                        self.lfo[i].phase
                    } else {
                        let cycles_per_beat = sync.cycles_per_beat();
                        (beats * cycles_per_beat).rem_euclid(1.0) as f32
                    }
                }
            };
            let new_phase = if new_phase_raw.is_finite() {
                new_phase_raw
            } else {
                0.0
            };

            // Detect cycle wrap to refresh S&H and Random targets.
            let wrapped = new_phase < self.lfo[i].phase;
            if wrapped {
                let sh = self.next_rand_bipolar();
                let new_b = self.next_rand_bipolar();
                let lfo = &mut self.lfo[i];
                lfo.sh_value = sh;
                lfo.rand_a = lfo.rand_b;
                lfo.rand_b = new_b;
            }
            self.lfo[i].phase = new_phase;

            let lfo = &self.lfo[i];
            out[i] = match cfg.shape {
                LfoShape::Sine => (lfo.phase * std::f32::consts::TAU).sin(),
                LfoShape::Triangle => {
                    let p = lfo.phase;
                    if p < 0.5 {
                        p * 4.0 - 1.0
                    } else {
                        3.0 - p * 4.0
                    }
                }
                LfoShape::Square => {
                    if lfo.phase < 0.5 {
                        1.0
                    } else {
                        -1.0
                    }
                }
                LfoShape::SampleHold => lfo.sh_value,
                LfoShape::Random => {
                    let t = lfo.phase;
                    let s = t * t * (3.0 - 2.0 * t);
                    lfo.rand_a * (1.0 - s) + lfo.rand_b * s
                }
            };
            // Suppress LFO output when target is Off so it can be muted
            // without zeroing depth (handy for previewing routing).
            if matches!(cfg.target, LfoTarget::Off) {
                out[i] = 0.0;
            }
            // Guard against any pathological shape arithmetic.
            if !out[i].is_finite() {
                out[i] = 0.0;
            }
        }
        out
    }

    fn next_rand_01(&mut self) -> f32 {
        self.rng = self.rng.wrapping_mul(1664525).wrapping_add(1013904223);
        ((self.rng >> 8) & 0x00FF_FFFF) as f32 / 0x0100_0000 as f32
    }

    fn next_rand_bipolar(&mut self) -> f32 {
        self.next_rand_01() * 2.0 - 1.0
    }

    pub fn process_sample(&mut self, in_l: f32, in_r: f32, p: &ParamValues) -> (f32, f32) {
        // Recover from non-finite state. NaN/Inf can sneak in via host edge
        // cases and would otherwise propagate forever through the smoothers
        // and the recirculating delay lines.
        if !(self.feedback_s.is_finite()
            && self.spread_s.is_finite()
            && self.drive_s.is_finite()
            && self.tone_s.is_finite()
            && self.wow_s.is_finite()
            && self.flutter_s.is_finite()
            && self.mix_s.is_finite()
            && self.ch_l.cur_delay.is_finite()
            && self.ch_r.cur_delay.is_finite())
        {
            self.feedback_s = 0.4;
            self.spread_s = 0.0;
            self.drive_s = 0.25;
            self.tone_s = 0.0;
            self.wow_s = 0.0;
            self.flutter_s = 0.0;
            self.mix_s = 0.5;
            self.ch_l = ChannelState::default();
            self.ch_r = ChannelState::default();
            self.snap_next = true;
            self.buffer_l.iter_mut().for_each(|s| *s = 0.0);
            self.buffer_r.iter_mut().for_each(|s| *s = 0.0);
        }

        // Sanitize host inputs before they hit the smoothers.
        let p_time = if p.time_ms.is_finite() {
            p.time_ms.clamp(MIN_DELAY_MS, MAX_DELAY_MS)
        } else {
            350.0
        };
        let p_fb = if p.feedback.is_finite() { p.feedback } else { 0.4 };
        let p_spread = if p.spread.is_finite() { p.spread } else { 0.0 };
        let p_drive = if p.drive.is_finite() { p.drive } else { 0.25 };
        let p_tone = if p.tone.is_finite() { p.tone } else { 0.0 };
        let p_wow = if p.wow.is_finite() { p.wow } else { 0.0 };
        let p_flutter = if p.flutter.is_finite() { p.flutter } else { 0.0 };
        let p_mix = if p.mix.is_finite() { p.mix } else { 0.5 };

        // Per-sample one-pole smoothing (same coefficient as STASIS).
        let smooth = 0.002;
        self.feedback_s += smooth * (p_fb - self.feedback_s);
        self.spread_s += smooth * (p_spread - self.spread_s);
        self.drive_s += smooth * (p_drive - self.drive_s);
        self.tone_s += smooth * (p_tone - self.tone_s);
        self.wow_s += smooth * (p_wow - self.wow_s);
        self.flutter_s += smooth * (p_flutter - self.flutter_s);
        self.mix_s += smooth * (p_mix - self.mix_s);

        let sr = self.sample_rate;
        let min_d = MIN_DELAY_MS * 0.001 * sr;
        // Reverse reads up to 2× the target behind the write head; keep the
        // target itself in the front half of the buffer with margin.
        let max_target = (self.buffer_samples as f32) * 0.5 - 8.0;
        let target_l = (p_time * 0.001 * sr).clamp(min_d, max_target);
        let spread = self.spread_s.clamp(0.0, 1.0);
        let target_r =
            (target_l * (1.0 + MAX_SPREAD_SKEW * spread)).clamp(min_d, max_target);

        if self.snap_next {
            self.snap_next = false;
            self.ch_l.cur_delay = target_l;
            self.ch_r.cur_delay = target_r;
            self.ch_l.rev_d = target_l;
            self.ch_r.rev_d = target_r;
        }

        // --- Wow & flutter: transport instability on the read head. ---
        // Wow is slow and scales with the delay time (a longer loop of tape
        // drifts further); flutter is a fast fixed-ms shimmer with a random
        // component. The right channel runs 90° behind on wow and has its
        // own flutter randomness so the image breathes instead of tracking.
        self.wow_phase += 0.45 / sr;
        if self.wow_phase >= 1.0 {
            self.wow_phase -= 1.0;
        }
        self.flutter_phase += 6.1 / sr;
        if self.flutter_phase >= 1.0 {
            self.flutter_phase -= 1.0;
        }
        if self.flutter_countdown <= 0 {
            self.flutter_countdown = (sr / 30.0) as i32;
            self.ch_l.flutter_target = self.next_rand_bipolar();
            self.ch_r.flutter_target = self.next_rand_bipolar();
        }
        self.flutter_countdown -= 1;
        self.ch_l.flutter_rand += 0.002 * (self.ch_l.flutter_target - self.ch_l.flutter_rand);
        self.ch_r.flutter_rand += 0.002 * (self.ch_r.flutter_target - self.ch_r.flutter_rand);

        let wow = self.wow_s.clamp(0.0, 1.0);
        let wow_amt = wow * wow;
        let flutter = self.flutter_s.clamp(0.0, 1.0);
        let flutter_amt = flutter * flutter;
        let wow_sin = (self.wow_phase * std::f32::consts::TAU).sin();
        let wow_cos = (self.wow_phase * std::f32::consts::TAU).cos();
        let flutter_sin = (self.flutter_phase * std::f32::consts::TAU).sin();

        let mod_l = wow_amt * (0.005 * self.ch_l.cur_delay + 0.0003 * sr) * wow_sin
            + flutter_amt * 0.0005 * sr * (0.65 * flutter_sin + 0.35 * self.ch_l.flutter_rand);
        let mod_r = wow_amt * (0.005 * self.ch_r.cur_delay + 0.0003 * sr) * wow_cos
            + flutter_amt
                * 0.0005
                * sr
                * (0.65 * -flutter_sin + 0.35 * self.ch_r.flutter_rand);

        // --- Per-channel time handling + tap read. ---
        let tape_alpha = 1.0 / (0.08 * sr); // ~80 ms slew time constant
        let fade_len = (0.04 * sr).max(64.0); // 40 ms digital crossfade
        let buf_len = self.buffer_samples;
        let write = self.write;

        let tap_l = read_tap(
            &self.buffer_l,
            buf_len,
            write,
            &mut self.ch_l,
            target_l,
            mod_l,
            p.mode,
            p.reverse,
            tape_alpha,
            fade_len,
            min_d,
            sr,
        );
        let tap_r = read_tap(
            &self.buffer_r,
            buf_len,
            write,
            &mut self.ch_r,
            target_r,
            mod_r,
            p.mode,
            p.reverse,
            tape_alpha,
            fade_len,
            min_d,
            sr,
        );

        // --- Feedback path coloration. ---
        // Hold bypasses the loop coloration entirely so the held loop
        // repeats without degrading, matching the hardware's clean hold.
        let (fb_l, fb_r) = if p.hold {
            (tap_l, tap_r)
        } else {
            let tone = self.tone_s.clamp(-1.0, 1.0);
            // Tone tilts the repeats: negative closes the lowpass down to
            // ~1.2 kHz, positive raises the highpass up to ~800 Hz. The
            // highpass floor of 25 Hz doubles as the loop's DC guard.
            let lp_base = 20000.0 * 0.06_f32.powf((-tone.min(0.0)) as f32);
            // Tape mode always loses a little top end per repeat.
            let lp_cut = if matches!(p.mode, DelayMode::Tape) {
                lp_base.min(11000.0)
            } else {
                lp_base
            };
            let hp_cut = 25.0 * 32.0_f32.powf(tone.max(0.0));
            let k_lp = 1.0 - (-std::f32::consts::TAU * lp_cut / sr).exp();
            let k_hp = 1.0 - (-std::f32::consts::TAU * hp_cut / sr).exp();
            let drive = self.drive_s.clamp(0.0, 1.0);

            (
                loop_color(tap_l, &mut self.ch_l, k_lp, k_hp, drive),
                loop_color(tap_r, &mut self.ch_r, k_lp, k_hp, drive),
            )
        };

        // --- Write the recirculating lines. ---
        // Hold: unity feedback, live input muted unless Add (overdub) is on.
        let eff_fb = if p.hold {
            1.0
        } else {
            self.feedback_s.clamp(0.0, 1.1)
        };
        let write_gain = if p.hold {
            if p.overdub {
                1.0
            } else {
                0.0
            }
        } else {
            1.0
        };

        let (w_l, w_r) = match p.mode {
            DelayMode::PingPong => {
                // Mono-summed input enters the left line; feedback crosses
                // so each repeat bounces to the other side.
                let mono = 0.5 * (in_l + in_r);
                (mono * write_gain + eff_fb * fb_r, eff_fb * fb_l)
            }
            _ => (
                in_l * write_gain + eff_fb * fb_l,
                in_r * write_gain + eff_fb * fb_r,
            ),
        };
        // Soft-clip at the write point — the "soft limiting compression"
        // that keeps runaway feedback blooming instead of exploding.
        self.buffer_l[write] = soft_clip(w_l);
        self.buffer_r[write] = soft_clip(w_r);
        self.write = (write + 1) % buf_len;

        // --- Output mix (constant power). ---
        let wet_l = soft_clip(tap_l);
        let wet_r = soft_clip(tap_r);
        let mix = self.mix_s.clamp(0.0, 1.0);
        let dry = (mix * std::f32::consts::FRAC_PI_2).cos();
        let wet = (mix * std::f32::consts::FRAC_PI_2).sin();

        let mut out_l = in_l * dry + wet_l * wet;
        let mut out_r = in_r * dry + wet_r * wet;

        if !out_l.is_finite() {
            out_l = 0.0;
        }
        if !out_r.is_finite() {
            out_r = 0.0;
        }

        // Publish for the GUI: effective read delays (with modulation) for
        // the TapeView tap markers, and the output scope.
        let ms = 1000.0 / sr;
        self.state.store_delay_ms(
            (self.ch_l.cur_delay + mod_l) * ms,
            (self.ch_r.cur_delay + mod_r) * ms,
        );
        self.state.scope_push((out_l + out_r) * 0.5);

        (out_l, out_r)
    }
}

/// Advance one channel's time state and read its (possibly reversed,
/// possibly crossfading) tap. Free function so the caller can hold `&mut`
/// channel state alongside a shared borrow of the sample buffer.
#[allow(clippy::too_many_arguments)]
fn read_tap(
    buf: &[f32],
    buf_len: usize,
    write: usize,
    ch: &mut ChannelState,
    target: f32,
    head_mod: f32,
    mode: DelayMode,
    reverse: bool,
    tape_alpha: f32,
    fade_len: f32,
    min_d: f32,
    sr: f32,
) -> f32 {
    match mode {
        DelayMode::Tape => {
            // Rate-limited exponential slew: the ±0.9 samples/sample cap
            // bounds the transposition to roughly ±1 octave down/up during
            // a swoop, like a motor that can only rev so fast.
            let step = (tape_alpha * (target - ch.cur_delay)).clamp(-0.9, 0.9);
            ch.cur_delay += step;
            ch.fading = false;
        }
        DelayMode::Digital | DelayMode::PingPong => {
            if ch.fading {
                ch.fade_pos += 1.0;
                if ch.fade_pos >= fade_len {
                    ch.fading = false;
                }
            } else if (target - ch.cur_delay).abs() > 2.0 {
                ch.fade_from = ch.cur_delay;
                ch.cur_delay = target;
                ch.fading = true;
                ch.fade_pos = 0.0;
            }
        }
    }

    let max_offset = buf_len as f32 - 8.0;
    let clamp_off = |o: f32| o.clamp(4.0, max_offset);

    if reverse {
        // Chunked backwards read: within each chunk of length rev_d the
        // read offset ramps 0 → 2·rev_d, i.e. the tap starts at "now" and
        // runs backwards through the last chunk of tape. Chunk length is
        // latched at each seam; seams get a short equal-power crossfade
        // against the previous chunk's continuation.
        if ch.rev_phase >= ch.rev_d || ch.rev_d < min_d * 0.5 {
            ch.rev_phase = if ch.rev_d > 0.0 {
                ch.rev_phase - ch.rev_d
            } else {
                0.0
            };
            ch.rev_d = ch.cur_delay.max(min_d);
        }
        let offset = clamp_off(2.0 * ch.rev_phase + head_mod);
        let tap_new = hermite_read(buf, buf_len, write as f32 - offset);
        let xf = (0.015 * sr).min(ch.rev_d * 0.5).max(1.0);
        let out = if ch.rev_phase < xf {
            let t = (ch.rev_phase / xf).clamp(0.0, 1.0);
            let old_off = clamp_off(2.0 * (ch.rev_phase + ch.rev_d) + head_mod);
            let tap_old = hermite_read(buf, buf_len, write as f32 - old_off);
            let a = (t * std::f32::consts::FRAC_PI_2).sin();
            let b = (t * std::f32::consts::FRAC_PI_2).cos();
            tap_new * a + tap_old * b
        } else {
            tap_new
        };
        ch.rev_phase += 1.0;
        return out;
    }

    let offset = clamp_off(ch.cur_delay + head_mod);
    let tap = hermite_read(buf, buf_len, write as f32 - offset);
    if ch.fading {
        let t = (ch.fade_pos / fade_len).clamp(0.0, 1.0);
        let old_off = clamp_off(ch.fade_from + head_mod);
        let tap_old = hermite_read(buf, buf_len, write as f32 - old_off);
        let a = (t * std::f32::consts::FRAC_PI_2).sin();
        let b = (t * std::f32::consts::FRAC_PI_2).cos();
        tap * a + tap_old * b
    } else {
        tap
    }
}

/// Feedback-loop coloration: tone lowpass → DC-safe highpass → tape drive.
/// The drive stage has unity small-signal gain (waveshaping only) plus a
/// modest level-dependent makeup, so the loop stays stable at feedback ≤ 1
/// and blooms into soft-clipped self-oscillation above it.
fn loop_color(x: f32, ch: &mut ChannelState, k_lp: f32, k_hp: f32, drive: f32) -> f32 {
    ch.lp += k_lp * (x - ch.lp);
    let x = ch.lp;
    ch.hp_lp += k_hp * (x - ch.hp_lp);
    let x = x - ch.hp_lp;

    let pre = 1.0 + 3.0 * drive;
    let x = soft_clip(x * pre) / pre * (1.0 + 0.5 * drive);
    soft_clip(x)
}

#[inline(always)]
fn hermite_read(buf: &[f32], len: usize, mut index: f32) -> f32 {
    let len_f = len as f32;
    while index < 0.0 {
        index += len_f;
    }
    while index >= len_f {
        index -= len_f;
    }

    let i1 = index as i32;
    let mut i0 = i1 - 1;
    let mut i2 = i1 + 1;
    let mut i3 = i1 + 2;
    let len_i = len as i32;

    if i0 < 0 {
        i0 += len_i;
    }
    if i2 >= len_i {
        i2 -= len_i;
    }
    if i3 >= len_i {
        i3 -= len_i;
    }

    let frac = index - i1 as f32;
    let y0 = buf[i0 as usize];
    let y1 = buf[i1 as usize];
    let y2 = buf[i2 as usize];
    let y3 = buf[i3 as usize];

    let c0 = y1;
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);

    ((c3 * frac + c2) * frac + c1) * frac + c0
}

#[inline(always)]
fn soft_clip(x: f32) -> f32 {
    if x > 1.5 {
        return 1.0;
    }
    if x < -1.5 {
        return -1.0;
    }
    x - (x * x * x) / 9.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DelayMode, LfoCfg, LfoShape, LfoTarget, ParamValues, SyncDivision};

    fn default_params() -> ParamValues {
        ParamValues {
            time_ms: 350.0,
            mode: DelayMode::Digital,
            feedback: 0.0,
            spread: 0.0,
            drive: 0.0,
            tone: 0.0,
            wow: 0.0,
            flutter: 0.0,
            mix: 1.0,
            input_gain_db: 0.0,
            output_gain_db: 0.0,
            hold: false,
            overdub: false,
            reverse: false,
        }
    }

    fn assert_finite(label: &str, l: f32, r: f32) {
        assert!(l.is_finite(), "{}: left non-finite ({})", label, l);
        assert!(r.is_finite(), "{}: right non-finite ({})", label, r);
    }

    fn pump<F: FnMut(usize) -> (f32, f32)>(
        e: &mut Engine,
        p: &ParamValues,
        n: usize,
        mut input: F,
    ) {
        for i in 0..n {
            let (l_in, r_in) = input(i);
            let (l, r) = e.process_sample(l_in, r_in, p);
            assert_finite("pump", l, r);
        }
    }

    #[test]
    fn init_allocates_buffers() {
        let mut e = Engine::new();
        e.init(48_000.0);
        assert_eq!(e.buffer_l.len(), e.buffer_samples);
        assert_eq!(e.buffer_r.len(), e.buffer_samples);
        assert!(e.buffer_samples > 0);
    }

    #[test]
    fn init_handles_invalid_sample_rate() {
        let mut e = Engine::new();
        e.init(0.0);
        assert!(e.sample_rate >= 48_000.0);
        assert!(e.buffer_samples > 0);
    }

    #[test]
    fn init_scales_with_sample_rate() {
        let mut e48 = Engine::new();
        e48.init(48_000.0);
        let mut e96 = Engine::new();
        e96.init(96_000.0);
        assert_eq!(e96.buffer_samples, e48.buffer_samples * 2);
    }

    #[test]
    fn silent_input_produces_finite_output() {
        let mut e = Engine::new();
        e.init(48_000.0);
        let p = default_params();
        pump(&mut e, &p, 4800, |_| (0.0, 0.0));
    }

    #[test]
    fn impulse_echoes_at_delay_time() {
        let mut e = Engine::new();
        e.init(48_000.0);
        let mut p = default_params();
        p.time_ms = 100.0; // 4800 samples
        p.mix = 1.0; // wet only

        let mut peak_idx = 0usize;
        let mut peak = 0.0f32;
        for i in 0..9600 {
            let x = if i == 0 { 1.0 } else { 0.0 };
            let (l, _r) = e.process_sample(x, x, &p);
            if l.abs() > peak {
                peak = l.abs();
                peak_idx = i;
            }
        }
        assert!(peak > 0.5, "echo should be strong, peak={}", peak);
        assert!(
            (peak_idx as i32 - 4800).abs() <= 4,
            "echo at {} expected ~4800",
            peak_idx
        );
    }

    #[test]
    fn feedback_produces_decaying_repeats() {
        let mut e = Engine::new();
        e.init(48_000.0);
        let mut p = default_params();
        p.time_ms = 50.0; // 2400 samples
        p.feedback = 0.5;
        p.mix = 1.0;

        let mut echo1 = 0.0f32;
        let mut echo2 = 0.0f32;
        for i in 0..7200 {
            let x = if i == 0 { 1.0 } else { 0.0 };
            let (l, _r) = e.process_sample(x, x, &p);
            if (2300..2500).contains(&i) {
                echo1 = echo1.max(l.abs());
            }
            if (4700..4900).contains(&i) {
                echo2 = echo2.max(l.abs());
            }
        }
        assert!(echo1 > 0.4, "first echo missing, {}", echo1);
        assert!(echo2 > 0.1, "second echo missing, {}", echo2);
        assert!(
            echo2 < echo1 * 0.75,
            "second echo should decay: {} vs {}",
            echo2,
            echo1
        );
    }

    #[test]
    fn ping_pong_alternates_sides() {
        let mut e = Engine::new();
        e.init(48_000.0);
        let mut p = default_params();
        p.mode = DelayMode::PingPong;
        p.time_ms = 50.0; // 2400 samples
        p.feedback = 0.7;
        p.mix = 1.0;

        let mut e1_l = 0.0f32;
        let mut e1_r = 0.0f32;
        let mut e2_l = 0.0f32;
        let mut e2_r = 0.0f32;
        for i in 0..7200 {
            let x = if i == 0 { 1.0 } else { 0.0 };
            let (l, r) = e.process_sample(x, x, &p);
            if (2300..2500).contains(&i) {
                e1_l = e1_l.max(l.abs());
                e1_r = e1_r.max(r.abs());
            }
            if (4700..4900).contains(&i) {
                e2_l = e2_l.max(l.abs());
                e2_r = e2_r.max(r.abs());
            }
        }
        assert!(e1_l > 0.3, "first echo should be on the left, {}", e1_l);
        assert!(e1_r < e1_l * 0.2, "first echo leaked right: {} vs {}", e1_r, e1_l);
        assert!(e2_r > 0.1, "second echo should be on the right, {}", e2_r);
        assert!(e2_l < e2_r * 0.3, "second echo leaked left: {} vs {}", e2_l, e2_r);
    }

    #[test]
    fn hold_sustains_the_loop_without_decay() {
        let mut e = Engine::new();
        e.init(48_000.0);
        let mut p = default_params();
        p.time_ms = 50.0; // 2400-sample loop
        p.feedback = 0.5;
        p.mix = 1.0;

        // Prime the line with a sine burst, then engage hold with silence.
        pump(&mut e, &p, 2400, |i| {
            let s = (i as f32 * 0.05).sin() * 0.5;
            (s, s)
        });
        p.hold = true;

        let mut cycle_peak = [0.0f32; 4];
        for c in 0..4 {
            for _ in 0..2400 {
                let (l, r) = e.process_sample(0.0, 0.0, &p);
                assert_finite("hold", l, r);
                cycle_peak[c] = cycle_peak[c].max(l.abs());
            }
        }
        assert!(cycle_peak[0] > 0.1, "held loop is silent");
        // Unity feedback + clean loop: later cycles keep the level.
        assert!(
            cycle_peak[3] > cycle_peak[0] * 0.9,
            "held loop decayed: {:?}",
            cycle_peak
        );
    }

    #[test]
    fn tape_mode_time_change_stays_finite_and_settles() {
        let mut e = Engine::new();
        e.init(48_000.0);
        let mut p = default_params();
        p.mode = DelayMode::Tape;
        p.time_ms = 100.0;
        pump(&mut e, &p, 4800, |i| ((i as f32 * 0.01).sin(), 0.0));

        p.time_ms = 800.0;
        pump(&mut e, &p, 96_000, |i| ((i as f32 * 0.01).sin(), 0.0));
        let target = 800.0 * 0.001 * 48_000.0;
        assert!(
            (e.ch_l.cur_delay - target).abs() < 48.0,
            "tape slew should settle near target: {} vs {}",
            e.ch_l.cur_delay,
            target
        );
    }

    #[test]
    fn reverse_mode_is_stable_and_audible() {
        let mut e = Engine::new();
        e.init(48_000.0);
        let mut p = default_params();
        p.time_ms = 200.0;
        p.reverse = true;
        p.feedback = 0.4;
        p.mix = 1.0;

        let mut peak = 0.0f32;
        for i in 0..48_000 {
            let s = (i as f32 * 0.02).sin() * 0.5;
            let (l, r) = e.process_sample(s, s, &p);
            assert_finite("reverse", l, r);
            peak = peak.max(l.abs());
        }
        assert!(peak > 0.05, "reverse tap should produce output, {}", peak);
    }

    #[test]
    fn high_feedback_with_drive_stays_bounded() {
        let mut e = Engine::new();
        e.init(48_000.0);
        let mut p = default_params();
        p.time_ms = 30.0;
        p.feedback = 1.1;
        p.drive = 1.0;
        p.mix = 1.0;

        let mut peak = 0.0f32;
        for i in 0..96_000 {
            let x = if i < 4800 { (i as f32 * 0.05).sin() } else { 0.0 };
            let (l, r) = e.process_sample(x, x, &p);
            assert_finite("selfosc", l, r);
            peak = peak.max(l.abs()).max(r.abs());
        }
        assert!(peak <= 1.05, "soft limiting violated, peak={}", peak);
    }

    #[test]
    fn spread_skews_right_channel_delay() {
        let mut e = Engine::new();
        e.init(48_000.0);
        let mut p = default_params();
        p.time_ms = 100.0;
        p.spread = 1.0;
        p.mix = 1.0;
        // Let the spread smoother settle, then re-snap timing state by
        // pumping long enough for digital retargeting to fire.
        pump(&mut e, &p, 48_000, |_| (0.0, 0.0));

        let mut peak_l = 0usize;
        let mut peak_r = 0usize;
        let mut max_l = 0.0f32;
        let mut max_r = 0.0f32;
        for i in 0..12_000 {
            let x = if i == 0 { 1.0 } else { 0.0 };
            let (l, r) = e.process_sample(x, x, &p);
            if l.abs() > max_l {
                max_l = l.abs();
                peak_l = i;
            }
            if r.abs() > max_r {
                max_r = r.abs();
                peak_r = i;
            }
        }
        assert!(
            peak_r > peak_l + 1000,
            "right echo should lag left: L={} R={}",
            peak_l,
            peak_r
        );
    }

    #[test]
    fn reset_clears_the_tail() {
        let mut e = Engine::new();
        e.init(48_000.0);
        let mut p = default_params();
        p.time_ms = 100.0;
        p.feedback = 0.9;
        p.mix = 1.0;
        pump(&mut e, &p, 9600, |i| ((i as f32 * 0.05).sin(), 0.0));
        e.reset();

        let mut peak = 0.0f32;
        for _ in 0..9600 {
            let (l, r) = e.process_sample(0.0, 0.0, &p);
            peak = peak.max(l.abs()).max(r.abs());
        }
        assert!(peak < 1e-4, "tail should be cleared after reset, {}", peak);
    }

    #[test]
    fn wow_flutter_stay_finite_at_extremes() {
        let mut e = Engine::new();
        e.init(48_000.0);
        let mut p = default_params();
        p.mode = DelayMode::Tape;
        p.time_ms = 500.0;
        p.wow = 1.0;
        p.flutter = 1.0;
        p.feedback = 0.8;
        p.mix = 1.0;
        pump(&mut e, &p, 96_000, |i| ((i as f32 * 0.01).sin() * 0.5, 0.0));
    }

    #[test]
    fn lfo_free_running_advances_phase() {
        let mut e = Engine::new();
        e.init(48_000.0);
        let cfgs = [
            LfoCfg {
                shape: LfoShape::Sine,
                sync: SyncDivision::Free,
                rate_hz: 1.0,
                depth: 1.0,
                target: LfoTarget::Time,
            },
            LfoCfg {
                shape: LfoShape::Triangle,
                sync: SyncDivision::Free,
                rate_hz: 2.0,
                depth: 1.0,
                target: LfoTarget::Feedback,
            },
        ];
        // 1 Hz over 0.25 s of blocks -> phase 0.25, sine hits +1.
        let mut out = [0.0f32; 2];
        for _ in 0..12 {
            out = e.step_lfos(None, None, 1000, &cfgs);
        }
        assert!(out[0].is_finite() && out[1].is_finite());
        assert!(e.lfo[0].phase > 0.2 && e.lfo[0].phase < 0.3);
    }

    #[test]
    fn lfo_sync_locks_to_beats() {
        let mut e = Engine::new();
        e.init(48_000.0);
        let cfgs = [
            LfoCfg {
                shape: LfoShape::Square,
                sync: SyncDivision::Quarter,
                rate_hz: 1.0,
                depth: 1.0,
                target: LfoTarget::Mix,
            },
            LfoCfg {
                shape: LfoShape::Sine,
                sync: SyncDivision::Free,
                rate_hz: 1.0,
                depth: 0.0,
                target: LfoTarget::Off,
            },
        ];
        // pos_beats = 0.25 with 1 cycle/beat -> phase 0.25 -> square = +1.
        let out = e.step_lfos(Some(120.0), Some(0.25), 64, &cfgs);
        assert_eq!(out[0], 1.0);
        // Non-finite pos_beats holds phase instead of going NaN.
        let out = e.step_lfos(Some(120.0), Some(f64::NAN), 64, &cfgs);
        assert!(out[0].is_finite());
    }

    #[test]
    fn soft_clip_bounds() {
        assert_eq!(soft_clip(2.0), 1.0);
        assert_eq!(soft_clip(-2.0), -1.0);
        assert!((soft_clip(0.1) - (0.1 - 0.001 / 9.0)).abs() < 1e-6);
    }

    #[test]
    fn hermite_interpolates_linearly_rising_signal() {
        let buf: Vec<f32> = (0..64).map(|i| i as f32).collect();
        let v = hermite_read(&buf, 64, 10.5);
        assert!((v - 10.5).abs() < 1e-3, "hermite at 10.5 = {}", v);
    }
}
