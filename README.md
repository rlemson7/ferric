# FERRIC

A stereo tape delay VST3/CLAP plugin built with [NIH-plug](https://github.com/robbert-vdh/nih-plug) and a [vizia](https://github.com/vizia/vizia)-based GUI. Inspired by the [Erica Synths Black Stereo Delay](https://www.ericasynths.lv/shop/eurorack-modules/by-series/black-series/black-stereo-delay/): tape / digital / ping-pong modes, feedback that blooms into soft-limited self-oscillation instead of exploding, hold with overdub, reverse, and wow & flutter. Sibling project to [STASIS](https://github.com/rlemson7/stasis) — same platform (presets, two GUI themes, 2 DAW-synced LFOs, GUI scale picker).

## Features

### DSP

- Delay time 3 ms – 3000 ms with the hardware's compressed low end
  (~190 ms at the center of the knob) for flange/chorus territory on the
  left half of the travel
- Host tempo sync: 1/32 … 1 bar including dotted and triplet divisions —
  the plugin-world equivalent of the hardware's tap/clock sync. The SYNC
  division wins over the TIME knob whenever the host provides a tempo.
- **Three delay modes:**
  - **Tape** — delay-time changes slew the read head (rate-limited to
    roughly ±1 octave of transposition) producing the classic pitch swoop,
    plus a fixed gentle top-end rolloff in the repeats
  - **Digital** — time changes are stepped 40 ms equal-power crossfades:
    clickless, but with the "musical digital artifacts" of a rack delay
  - **Ping-Pong** — mono-summed input feeds the left line and feedback
    crosses L→R→L so repeats bounce between speakers
- Feedback 0–110 %: the loop has soft-limiting at the write point, so past
  unity it blooms into bounded self-oscillation, like the hardware past
  12 o'clock
- Feedback-path coloration: TONE tilt (negative closes a lowpass down to
  ~1.2 kHz, positive raises a highpass up to ~800 Hz; the 25 Hz highpass
  floor doubles as the loop's DC guard) and DRIVE tape saturation with
  unity small-signal gain so the loop stays stable
- **Hold** freezes the loop at unity feedback with the coloration stage
  bypassed, so the held loop repeats cleanly without degrading; **Add**
  overdubs live input into the held loop
- **Reverse** plays the delay tail backwards in delay-length chunks with an
  equal-power crossfade at each chunk seam. During Hold the loop keeps
  recirculating forward underneath, so you hear the held loop backwards
  while its content stays intact — toggling Reverse off returns the
  original loop.
- Wow (0.45 Hz, depth scales with delay time, right channel 90° behind) &
  flutter (6.1 Hz sine + smoothed random, independent per channel) modulate
  the read head like a worn transport
- SPREAD skews the right channel's delay time up to +35 % for stereo
  widening / rhythmic offsets
- Hermite cubic interpolation on all tap reads; constant-power dry/wet mix;
  ±24 dB input and output trims; per-sample one-pole parameter smoothing

### Modulation

- 2 internal LFOs, each independently routable to one parameter
  (Time, Feedback, Spread, Tone, Drive, Wow, Flutter, Mix, In/Out Gain)
- Shapes: Sine, Triangle, Square, Sample & Hold, Smooth Random
- DAW sync to host transport (`pos_beats`): 1/16 … 4 bars; free-running
  mode uses a 0.05–30 Hz rate slider
- Time modulation is multiplicative (±1 octave at full depth), so small
  depths give chorus/flange wobble at any base delay time

### GUI

- Custom vector logo (tape-reel icon mark + FERRIC wordmark) drawn via
  femtovg so it stays crisp across the full GUI scale range
- **TapeView** — live L/R transport lanes: the write head sits at the left
  edge, the read tap marker sits at its current *effective* delay (it
  physically swoops during tape-mode time changes and wanders with wow &
  flutter), and ghost markers show each feedback repeat fading at the
  feedback amount. Hold pins the ghosts at full brightness; reverse adds a
  direction wedge.
- Real-time output oscilloscope with CRT-style phosphor graticule
- Preset picker + Save (JSON `.ferric` files in
  `~/Library/Audio/Presets/Realtime Media/FERRIC/` on macOS); index 0 is
  always **Init Patch**
- Two themes (Classic / Terminal) via the ⋯ settings modal
- Stepped GUI scale picker (0.75× .. 2.0×) — applies on next plugin window
  open (workaround for Live's VST3 host denying mid-session resize)

## Parameters

| ID | Name | Range | Default | Notes |
| --- | --- | --- | --- | --- |
| `time` | Time | 3..3000 ms (skewed) | 350 ms | Ignored when SYNC ≠ Free and the host has a tempo |
| `sync` | Sync | Free / 1/32 .. 1 bar | Free | Dotted + triplet divisions included |
| `mode` | Mode | Tape / Digital / Ping-Pong | Tape | |
| `feedback` | Feedback | 0..110 % | 40 % | >100 % = soft-limited self-oscillation |
| `spread` | Spread | 0..1 | 0 | Right-channel delay skew, up to +35 % |
| `drive` | Drive | 0..1 | 0.25 | Tape saturation in the feedback loop |
| `tone` | Tone | -1..+1 | 0 | Dark ← → bright tilt on the repeats |
| `wow` | Wow | 0..1 | 0.15 | Slow transport drift |
| `flutter` | Flutter | 0..1 | 0.10 | Fast transport shimmer |
| `mix` | Mix | 0..1 | 0.5 | Dry/wet, constant-power |
| `ingain` / `outgain` | In/Out Gain | -24..+24 dB | 0 | Trims around the engine |
| `hold` | Hold | bool | false | Freeze the loop, unity feedback, clean repeats |
| `add` | Add | bool | false | Overdub live input into the held loop |
| `reverse` | Reverse | bool | false | Backwards chunked read of the tail |
| `lfo{1,2}_*` | LFO 1/2 | shape / sync / rate / depth / target | — | Same modulation platform as STASIS |
| `scale` | GUI Scale | 0.75..2.0 step 0.25 | 1.5 | Takes effect on next window open |

## Architecture

- **Engine** (`src/engine.rs`) — pure DSP: one 8.5 s stereo circular buffer,
  per-channel time state (tape slew / digital crossfade / reverse chunk
  phase), feedback coloration, wow & flutter, two LFO oscillators. Lock-free
  audio↔GUI communication via atomics only. Includes in-tree smoke tests.
- **Shared state** (`src/state.rs`) — scope ring buffer, effective per-channel
  delay (for the TapeView), sample rate, theme index.
- **Presets** (`src/preset.rs`) — preset folder, JSON serialization via
  `Params::serialize_fields` / `deserialize_fields`.
- **Editor** (`src/editor.rs`, `src/editor.css`) — vizia layout with three
  custom widgets: `TapeView`, `ScopeView`, `LogoView`. A 30 Hz redraw timer
  drives the marker/scope animation.

## Build

```
cargo build --release
cargo test
```

For VST3/CLAP bundles using NIH-plug's bundler:

```
cargo install --git https://github.com/robbert-vdh/nih-plug.git cargo-nih-plug
cargo nih-plug bundle ferric --release
```

The resulting `.vst3` and `.clap` end up in `target/bundled/`.

### Quick install (macOS)

```
./install.sh
```

Builds + bundles + copies into `~/Library/Audio/Plug-Ins/{VST3,CLAP}/` and
strips quarantine attrs so Gatekeeper doesn't block local loads. Quit and
restart your DAW after running this.

## License

GPL-3.0-or-later — required when distributing a VST3 built against
Steinberg's open-source SDK terms (which NIH-plug wraps).
