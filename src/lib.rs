use nih_plug::prelude::*;
use std::any::Any;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

mod editor;
mod engine;
pub mod preset;
pub mod state;
use engine::Engine;
use state::SharedState;

pub const DESIGN_WIDTH: u32 = 760;
pub const DESIGN_HEIGHT: u32 = 690;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum DelayMode {
    Tape,
    Digital,
    #[name = "Ping-Pong"]
    PingPong,
}

/// Host-sync divisions for the delay time. `Free` uses the TIME knob;
/// everything else derives the time from the host tempo (the plugin-world
/// equivalent of the hardware's tap/clock sync).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum DelaySync {
    Free,
    #[name = "1/32"]
    ThirtySecond,
    #[name = "1/16"]
    Sixteenth,
    #[name = "1/16."]
    SixteenthDotted,
    #[name = "1/8T"]
    EighthTriplet,
    #[name = "1/8"]
    Eighth,
    #[name = "1/8."]
    EighthDotted,
    #[name = "1/4T"]
    QuarterTriplet,
    #[name = "1/4"]
    Quarter,
    #[name = "1/4."]
    QuarterDotted,
    #[name = "1/2"]
    Half,
    #[name = "1 bar"]
    OneBar,
}

impl DelaySync {
    /// Length of the division in beats (a beat = quarter note).
    pub fn beats(self) -> f64 {
        match self {
            DelaySync::Free => 0.0,
            DelaySync::ThirtySecond => 0.125,
            DelaySync::Sixteenth => 0.25,
            DelaySync::SixteenthDotted => 0.375,
            DelaySync::EighthTriplet => 1.0 / 3.0,
            DelaySync::Eighth => 0.5,
            DelaySync::EighthDotted => 0.75,
            DelaySync::QuarterTriplet => 2.0 / 3.0,
            DelaySync::Quarter => 1.0,
            DelaySync::QuarterDotted => 1.5,
            DelaySync::Half => 2.0,
            DelaySync::OneBar => 4.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum LfoShape {
    Sine,
    Triangle,
    Square,
    #[name = "S&H"]
    SampleHold,
    Random,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum SyncDivision {
    Free,
    #[name = "1/16"]
    Sixteenth,
    #[name = "1/8"]
    Eighth,
    #[name = "1/4"]
    Quarter,
    #[name = "1/2"]
    Half,
    #[name = "1 bar"]
    OneBar,
    #[name = "2 bars"]
    TwoBars,
    #[name = "4 bars"]
    FourBars,
}

impl SyncDivision {
    /// Number of full LFO cycles per beat (a beat = quarter note).
    pub fn cycles_per_beat(self) -> f64 {
        match self {
            SyncDivision::Free => 0.0,
            SyncDivision::Sixteenth => 4.0,
            SyncDivision::Eighth => 2.0,
            SyncDivision::Quarter => 1.0,
            SyncDivision::Half => 0.5,
            SyncDivision::OneBar => 0.25,
            SyncDivision::TwoBars => 0.125,
            SyncDivision::FourBars => 0.0625,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum LfoTarget {
    Off,
    Time,
    Feedback,
    Spread,
    Tone,
    Drive,
    Wow,
    Flutter,
    Mix,
    #[name = "In Gain"]
    InputGain,
    #[name = "Out Gain"]
    OutputGain,
}

/// Resolved per-block parameter values handed to the engine. `time_ms` is
/// already sync-resolved (knob or tempo division) and LFO-modulated.
pub struct ParamValues {
    pub time_ms: f32,
    pub mode: DelayMode,
    pub feedback: f32,
    pub spread: f32,
    pub drive: f32,
    pub tone: f32,
    pub wow: f32,
    pub flutter: f32,
    pub mix: f32,
    pub input_gain_db: f32,
    pub output_gain_db: f32,
    pub hold: bool,
    pub overdub: bool,
    pub reverse: bool,
}

#[derive(Clone, Copy)]
pub struct LfoCfg {
    pub shape: LfoShape,
    pub sync: SyncDivision,
    pub rate_hz: f32,
    pub depth: f32,
    pub target: LfoTarget,
}

struct Ferric {
    params: Arc<FerricParams>,
    state: Arc<SharedState>,
    engine: Engine,
}

#[derive(Params)]
pub struct FerricParams {
    /// Persisted GUI scale. Stepped 0.75×..2.0× in 0.25 increments. The
    /// editor is created at this scale via `ViziaState::new_with_default_
    /// scale_factor`, so changing the value takes effect when the plugin
    /// window is closed and reopened. Live's VST3 host denies the runtime
    /// resize-on-drag handshake, so a static scale is the workable path.
    #[id = "scale"]
    pub gui_scale: FloatParam,

    #[id = "time"]
    pub time_ms: FloatParam,
    #[id = "sync"]
    pub sync: EnumParam<DelaySync>,
    #[id = "mode"]
    pub mode: EnumParam<DelayMode>,
    #[id = "feedback"]
    pub feedback: FloatParam,
    #[id = "spread"]
    pub spread: FloatParam,

    #[id = "drive"]
    pub drive: FloatParam,
    #[id = "tone"]
    pub tone: FloatParam,
    #[id = "wow"]
    pub wow: FloatParam,
    #[id = "flutter"]
    pub flutter: FloatParam,

    #[id = "mix"]
    pub mix: FloatParam,
    #[id = "ingain"]
    pub input_gain_db: FloatParam,
    #[id = "outgain"]
    pub output_gain_db: FloatParam,

    #[id = "hold"]
    pub hold: BoolParam,
    #[id = "add"]
    pub overdub: BoolParam,
    #[id = "reverse"]
    pub reverse: BoolParam,

    #[id = "lfo1shp"]
    pub lfo1_shape: EnumParam<LfoShape>,
    #[id = "lfo1syn"]
    pub lfo1_sync: EnumParam<SyncDivision>,
    #[id = "lfo1rt"]
    pub lfo1_rate_hz: FloatParam,
    #[id = "lfo1dpt"]
    pub lfo1_depth: FloatParam,
    #[id = "lfo1tgt"]
    pub lfo1_target: EnumParam<LfoTarget>,

    #[id = "lfo2shp"]
    pub lfo2_shape: EnumParam<LfoShape>,
    #[id = "lfo2syn"]
    pub lfo2_sync: EnumParam<SyncDivision>,
    #[id = "lfo2rt"]
    pub lfo2_rate_hz: FloatParam,
    #[id = "lfo2dpt"]
    pub lfo2_depth: FloatParam,
    #[id = "lfo2tgt"]
    pub lfo2_target: EnumParam<LfoTarget>,
}

impl Default for Ferric {
    fn default() -> Self {
        let params = Arc::new(FerricParams::default());
        let state = Arc::new(SharedState::new());
        let engine = Engine::with_state(state.clone());
        Self {
            params,
            state,
            engine,
        }
    }
}

impl Default for FerricParams {
    fn default() -> Self {
        let unit_range = FloatRange::Linear { min: 0.0, max: 1.0 };
        let lfo_rate_range = FloatRange::Skewed {
            min: 0.05,
            max: 30.0,
            factor: FloatRange::skew_factor(-2.0),
        };
        let gain_range = FloatRange::Linear { min: -24.0, max: 24.0 };

        Self {
            gui_scale: FloatParam::new(
                "GUI Scale",
                1.5,
                FloatRange::Linear { min: 0.75, max: 2.0 },
            )
            .with_step_size(0.25)
            .with_value_to_string(Arc::new(|v| format!("{:.2}\u{00d7}", v)))
            .with_string_to_value(Arc::new(|s| {
                s.trim()
                    .trim_end_matches(|c: char| !c.is_ascii_digit() && c != '.')
                    .parse::<f32>()
                    .ok()
            })),

            // 3 ms .. 3000 ms with the hardware's compressed low end —
            // ~190 ms sits at the center of the knob's travel.
            time_ms: FloatParam::new(
                "Time",
                350.0,
                FloatRange::Skewed {
                    min: engine::MIN_DELAY_MS,
                    max: engine::MAX_DELAY_MS,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_unit(" ms")
            .with_value_to_string(formatters::v2s_f32_rounded(0)),
            sync: EnumParam::new("Sync", DelaySync::Free),
            mode: EnumParam::new("Mode", DelayMode::Tape),
            // Up to 110%: past unity the loop blooms into (soft-limited)
            // self-oscillation, like the hardware past 12 o'clock.
            feedback: FloatParam::new(
                "Feedback",
                0.4,
                FloatRange::Linear { min: 0.0, max: 1.1 },
            )
            .with_unit(" %")
            .with_value_to_string(Arc::new(|v| format!("{:.0}", v * 100.0)))
            .with_string_to_value(Arc::new(|s| {
                s.trim().trim_end_matches('%').trim().parse::<f32>().ok().map(|v| v / 100.0)
            })),
            spread: FloatParam::new("Spread", 0.0, unit_range),

            drive: FloatParam::new("Drive", 0.25, unit_range),
            tone: FloatParam::new("Tone", 0.0, FloatRange::Linear { min: -1.0, max: 1.0 })
                .with_value_to_string(Arc::new(|v| format!("{:+.2}", v))),
            wow: FloatParam::new("Wow", 0.15, unit_range),
            flutter: FloatParam::new("Flutter", 0.10, unit_range),

            mix: FloatParam::new("Mix", 0.5, unit_range),
            input_gain_db: FloatParam::new("Input Gain", 0.0, gain_range)
                .with_unit(" dB")
                .with_step_size(0.1)
                .with_value_to_string(formatters::v2s_f32_rounded(1)),
            output_gain_db: FloatParam::new("Output Gain", 0.0, gain_range)
                .with_unit(" dB")
                .with_step_size(0.1)
                .with_value_to_string(formatters::v2s_f32_rounded(1)),

            hold: BoolParam::new("Hold", false),
            overdub: BoolParam::new("Add", false),
            reverse: BoolParam::new("Reverse", false),

            lfo1_shape: EnumParam::new("LFO1 Shape", LfoShape::Sine),
            lfo1_sync: EnumParam::new("LFO1 Sync", SyncDivision::Free),
            lfo1_rate_hz: FloatParam::new("LFO1 Rate", 1.0, lfo_rate_range)
                .with_unit(" Hz")
                .with_value_to_string(formatters::v2s_f32_rounded(2)),
            lfo1_depth: FloatParam::new("LFO1 Depth", 0.0, unit_range),
            lfo1_target: EnumParam::new("LFO1 Target", LfoTarget::Off),

            lfo2_shape: EnumParam::new("LFO2 Shape", LfoShape::Triangle),
            lfo2_sync: EnumParam::new("LFO2 Sync", SyncDivision::Free),
            lfo2_rate_hz: FloatParam::new("LFO2 Rate", 0.5, lfo_rate_range)
                .with_unit(" Hz")
                .with_value_to_string(formatters::v2s_f32_rounded(2)),
            lfo2_depth: FloatParam::new("LFO2 Depth", 0.0, unit_range),
            lfo2_target: EnumParam::new("LFO2 Target", LfoTarget::Off),
        }
    }
}

impl Plugin for Ferric {
    const NAME: &'static str = "FERRIC";
    const VENDOR: &'static str = "Realtime Media";
    const URL: &'static str = "";
    const EMAIL: &'static str = "rieko@realtime-media.nl";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: NonZeroU32::new(2),
        main_output_channels: NonZeroU32::new(2),
        ..AudioIOLayout::const_default()
    }];

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        // NIH-plug calls Plugin::editor() exactly once per plugin instance,
        // before setState restores persisted params. That means reading
        // gui_scale here always sees the default. To make the user-picked
        // scale actually apply (after save/reload OR after closing and
        // reopening the GUI), wrap the inner editor in an adapter that
        // re-creates the underlying ViziaEditor on every spawn() with the
        // *current* param value.
        Some(Box::new(DynamicScaleEditor {
            params: self.params.clone(),
            state: self.state.clone(),
            inner: Mutex::new(None),
        }))
    }

    fn initialize(
        &mut self,
        _layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        self.engine.init(buffer_config.sample_rate);
        true
    }

    fn reset(&mut self) {
        self.engine.reset();
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let transport = context.transport();
        let block_size = buffer.samples();

        let lfo1_cfg = LfoCfg {
            shape: self.params.lfo1_shape.value(),
            sync: self.params.lfo1_sync.value(),
            rate_hz: self.params.lfo1_rate_hz.value(),
            depth: self.params.lfo1_depth.value(),
            target: self.params.lfo1_target.value(),
        };
        let lfo2_cfg = LfoCfg {
            shape: self.params.lfo2_shape.value(),
            sync: self.params.lfo2_sync.value(),
            rate_hz: self.params.lfo2_rate_hz.value(),
            depth: self.params.lfo2_depth.value(),
            target: self.params.lfo2_target.value(),
        };

        let lfo_outputs = self.engine.step_lfos(
            transport.tempo,
            transport.pos_beats(),
            block_size,
            &[lfo1_cfg, lfo2_cfg],
        );

        // Resolve delay time: the SYNC division wins over the TIME knob
        // whenever the host provides a tempo.
        let sync = self.params.sync.value();
        let time_ms = match (sync, transport.tempo) {
            (DelaySync::Free, _) | (_, None) => self.params.time_ms.value(),
            (div, Some(bpm)) if bpm > 0.0 => {
                (div.beats() * 60_000.0 / bpm) as f32
            }
            _ => self.params.time_ms.value(),
        }
        .clamp(engine::MIN_DELAY_MS, engine::MAX_DELAY_MS);

        let mut p = ParamValues {
            time_ms,
            mode: self.params.mode.value(),
            feedback: self.params.feedback.value(),
            spread: self.params.spread.value(),
            drive: self.params.drive.value(),
            tone: self.params.tone.value(),
            wow: self.params.wow.value(),
            flutter: self.params.flutter.value(),
            mix: self.params.mix.value(),
            input_gain_db: self.params.input_gain_db.value(),
            output_gain_db: self.params.output_gain_db.value(),
            hold: self.params.hold.value(),
            overdub: self.params.overdub.value(),
            reverse: self.params.reverse.value(),
        };

        apply_lfo(&mut p, lfo_outputs[0], lfo1_cfg);
        apply_lfo(&mut p, lfo_outputs[1], lfo2_cfg);

        let in_lin = util::db_to_gain(p.input_gain_db);
        let out_lin = util::db_to_gain(p.output_gain_db);

        for mut channels in buffer.iter_samples() {
            let mut iter = channels.iter_mut();
            let (Some(l), Some(r)) = (iter.next(), iter.next()) else {
                continue;
            };
            let (out_l, out_r) =
                self.engine.process_sample(*l * in_lin, *r * in_lin, &p);
            *l = out_l * out_lin;
            *r = out_r * out_lin;
        }

        ProcessStatus::Normal
    }
}

/// Apply an LFO output (`lfo` ∈ [-1, 1], `depth` ∈ [0, 1]) to the configured
/// target on `p`. Modulation is bipolar centered on the host value: ±half
/// the range for linear params, ±12 dB for the gain trims. Time modulates
/// multiplicatively (±1 octave at full depth) so small depths give chorus/
/// flange wobble at any base time instead of a fixed ±1.5 s.
fn apply_lfo(p: &mut ParamValues, lfo: f32, cfg: LfoCfg) {
    if cfg.depth <= 0.0 || !lfo.is_finite() {
        return;
    }
    let m = lfo * cfg.depth;
    match cfg.target {
        LfoTarget::Off => {}
        LfoTarget::Time => {
            p.time_ms = (p.time_ms * 2.0_f32.powf(m))
                .clamp(engine::MIN_DELAY_MS, engine::MAX_DELAY_MS)
        }
        LfoTarget::Feedback => p.feedback = (p.feedback + m * 0.55).clamp(0.0, 1.1),
        LfoTarget::Spread => p.spread = (p.spread + m * 0.5).clamp(0.0, 1.0),
        LfoTarget::Tone => p.tone = (p.tone + m).clamp(-1.0, 1.0),
        LfoTarget::Drive => p.drive = (p.drive + m * 0.5).clamp(0.0, 1.0),
        LfoTarget::Wow => p.wow = (p.wow + m * 0.5).clamp(0.0, 1.0),
        LfoTarget::Flutter => p.flutter = (p.flutter + m * 0.5).clamp(0.0, 1.0),
        LfoTarget::Mix => p.mix = (p.mix + m * 0.5).clamp(0.0, 1.0),
        LfoTarget::InputGain => {
            p.input_gain_db = (p.input_gain_db + m * 12.0).clamp(-24.0, 24.0)
        }
        LfoTarget::OutputGain => {
            p.output_gain_db = (p.output_gain_db + m * 12.0).clamp(-24.0, 24.0)
        }
    }
}

/// Adapter that creates a fresh `ViziaEditor` (with the current `gui_scale`)
/// every time the host opens the GUI. Sidesteps the fact that NIH-plug's
/// `Plugin::editor()` is called once per instance, before `setState`, which
/// would otherwise lock the plugin to its default scale.
struct DynamicScaleEditor {
    params: Arc<FerricParams>,
    state: Arc<SharedState>,
    inner: Mutex<Option<Box<dyn Editor>>>,
}

impl Editor for DynamicScaleEditor {
    fn spawn(
        &self,
        parent: ParentWindowHandle,
        context: Arc<dyn GuiContext>,
    ) -> Box<dyn Any + Send> {
        let scale = self.params.gui_scale.value() as f64;
        let new_inner = editor::create(
            self.params.clone(),
            editor::make_state(scale),
            self.state.clone(),
        )
        .expect("vizia editor should always be createable");

        let handle = new_inner.spawn(parent, context);
        *self.inner.lock().unwrap() = Some(new_inner);
        handle
    }

    fn size(&self) -> (u32, u32) {
        let scale = self.params.gui_scale.value() as f64;
        (
            (DESIGN_WIDTH as f64 * scale).round() as u32,
            (DESIGN_HEIGHT as f64 * scale).round() as u32,
        )
    }

    fn set_scale_factor(&self, factor: f32) -> bool {
        match self.inner.lock().unwrap().as_ref() {
            Some(inner) => inner.set_scale_factor(factor),
            None => true,
        }
    }

    fn param_value_changed(&self, id: &str, normalized_value: f32) {
        if let Some(inner) = self.inner.lock().unwrap().as_ref() {
            inner.param_value_changed(id, normalized_value);
        }
    }

    fn param_modulation_changed(&self, id: &str, modulation_offset: f32) {
        if let Some(inner) = self.inner.lock().unwrap().as_ref() {
            inner.param_modulation_changed(id, modulation_offset);
        }
    }

    fn param_values_changed(&self) {
        if let Some(inner) = self.inner.lock().unwrap().as_ref() {
            inner.param_values_changed();
        }
    }
}

impl ClapPlugin for Ferric {
    const CLAP_ID: &'static str = "nl.realtime-media.ferric";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("FERRIC — stereo tape delay with tape/digital/ping-pong modes, hold and reverse");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Stereo,
        ClapFeature::Delay,
    ];
}

impl Vst3Plugin for Ferric {
    const VST3_CLASS_ID: [u8; 16] = *b"FerricRMv0010001";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Delay];
}

nih_export_clap!(Ferric);
nih_export_vst3!(Ferric);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_division_beat_math() {
        assert_eq!(DelaySync::Quarter.beats(), 1.0);
        assert_eq!(DelaySync::OneBar.beats(), 4.0);
        assert!((DelaySync::EighthTriplet.beats() - 1.0 / 3.0).abs() < 1e-12);
        // 1/8 at 120 BPM = 250 ms.
        let ms = DelaySync::Eighth.beats() * 60_000.0 / 120.0;
        assert!((ms - 250.0).abs() < 1e-9);
    }

    #[test]
    fn lfo_time_modulation_is_multiplicative_and_clamped() {
        let mut p = ParamValues {
            time_ms: 400.0,
            mode: DelayMode::Tape,
            feedback: 0.4,
            spread: 0.0,
            drive: 0.0,
            tone: 0.0,
            wow: 0.0,
            flutter: 0.0,
            mix: 0.5,
            input_gain_db: 0.0,
            output_gain_db: 0.0,
            hold: false,
            overdub: false,
            reverse: false,
        };
        let cfg = LfoCfg {
            shape: LfoShape::Sine,
            sync: SyncDivision::Free,
            rate_hz: 1.0,
            depth: 1.0,
            target: LfoTarget::Time,
        };
        apply_lfo(&mut p, 1.0, cfg);
        assert!((p.time_ms - 800.0).abs() < 1e-3);
        apply_lfo(&mut p, -1.0, cfg);
        assert!((p.time_ms - 400.0).abs() < 1e-3);

        p.time_ms = 2900.0;
        apply_lfo(&mut p, 1.0, cfg);
        assert_eq!(p.time_ms, engine::MAX_DELAY_MS);
    }
}
