use nih_plug::prelude::{Editor, Param, ParamPtr, Params};
use nih_plug_vizia::vizia::prelude::*;
use nih_plug_vizia::vizia::vg::{Color as VgColor, Paint, Path};
use nih_plug_vizia::widgets::*;
use nih_plug_vizia::{assets, create_vizia_editor, ViziaState, ViziaTheming};
use std::sync::Arc;
use std::time::Duration;

use crate::state::SharedState;
use crate::{preset, FerricParams};

const STYLE: &str = include_str!("editor.css");

#[derive(Lens)]
struct AppData {
    params: Arc<FerricParams>,
    shared: Arc<SharedState>,
    preset_state: PresetState,
    /// Bound to the preset-name Textbox. The Save button writes to
    /// `<preset_dir>/<save_name>.ferric`. Empty name means no save.
    save_name: String,
    /// Active visual theme. Toggled at runtime by re-evaluating the
    /// `.theme-*` class on the root view; CSS overrides cascade from there.
    theme: Theme,
    /// Whether the settings modal is open.
    show_settings: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Theme {
    Classic,
    Terminal,
}

impl Data for Theme {
    fn same(&self, other: &Self) -> bool {
        self == other
    }
}

/// The slot at index 0 in `list` is always "Init Patch" (synthetic — not a
/// real file). Indices 1..n correspond to discovered preset files.
#[derive(Lens, Clone, Data)]
struct PresetState {
    list: Vec<String>,
    current_index: usize,
    current_name: String,
}

enum PresetEvent {
    LoadByIdx(usize),
    EditSaveName(String),
    SaveAs,
}

enum SettingsEvent {
    Toggle,
    SetTheme(Theme),
}

/// Set one parameter through the GUI's raw param events — the host sees a
/// proper begin/set/end gesture, so automation and undo behave.
fn set_param(cx: &mut EventContext, ptr: ParamPtr, normalized: f32) {
    cx.emit(RawParamEvent::BeginSetParameter(ptr));
    cx.emit(RawParamEvent::SetParameterNormalized(
        ptr,
        normalized.clamp(0.0, 1.0),
    ));
    cx.emit(RawParamEvent::EndSetParameter(ptr));
}

/// Apply a preset's id → plain-value map onto the live parameters.
/// Unknown ids and unparseable values are skipped, so presets from newer
/// plugin versions degrade gracefully. GUI-scale (and any other id in
/// `preset::SKIP_IDS`) is never touched.
fn apply_preset(cx: &mut EventContext, params: &FerricParams, map: &std::collections::BTreeMap<String, String>) {
    for (id, ptr, _group) in params.param_map() {
        if preset::SKIP_IDS.contains(&id.as_str()) {
            continue;
        }
        let Some(raw) = map.get(&id) else {
            continue;
        };
        let Ok(plain) = raw.trim().parse::<f32>() else {
            nih_plug::nih_warn!("preset: bad value for {id}: {raw}");
            continue;
        };
        if !plain.is_finite() {
            continue;
        }
        // Safety: `ptr` points into `params`, which the editor holds an
        // `Arc` to for its whole lifetime.
        let normalized = unsafe { ptr.preview_normalized(plain) };
        set_param(cx, ptr, normalized);
    }
}

/// Reset every parameter (except the skipped ids) to its declared default.
fn apply_defaults(cx: &mut EventContext, params: &FerricParams) {
    for (id, ptr, _group) in params.param_map() {
        if preset::SKIP_IDS.contains(&id.as_str()) {
            continue;
        }
        let normalized = unsafe { ptr.default_normalized_value() };
        set_param(cx, ptr, normalized);
    }
}

impl Model for AppData {
    fn event(&mut self, cx: &mut EventContext, event: &mut Event) {
        event.map(|e: &SettingsEvent, _| match e {
            SettingsEvent::Toggle => self.show_settings = !self.show_settings,
            SettingsEvent::SetTheme(t) => {
                self.theme = *t;
                // Mirror to SharedState so the custom-drawn widgets
                // (TapeView, ScopeView) can pick up the new theme.
                let v: u8 = match *t {
                    Theme::Classic => 0,
                    Theme::Terminal => 1,
                };
                self.shared
                    .theme
                    .store(v, std::sync::atomic::Ordering::Relaxed);
            }
        });
        event.map(|e: &PresetEvent, _| match e {
            PresetEvent::LoadByIdx(idx) => {
                if *idx == 0 {
                    apply_defaults(cx, &self.params);
                    self.preset_state.current_index = 0;
                    self.preset_state.current_name = "Init Patch".to_string();
                } else {
                    let paths = preset::list_presets();
                    let preset_idx = idx - 1;
                    if let Some(path) = paths.get(preset_idx) {
                        match preset::read_preset(path) {
                            Ok(map) => apply_preset(cx, &self.params, &map),
                            Err(err) => {
                                nih_plug::nih_warn!("preset load failed: {err}");
                                return;
                            }
                        }
                        self.preset_state.current_index = *idx;
                        self.preset_state.current_name = preset::preset_name(path);
                    }
                }
            }
            PresetEvent::EditSaveName(s) => {
                self.save_name = s.clone();
            }
            PresetEvent::SaveAs => {
                let raw = self.save_name.trim();
                if raw.is_empty() {
                    return;
                }
                // Strip a trailing ".ferric" if the user typed it.
                let name = raw.strip_suffix(".ferric").unwrap_or(raw);
                let dir = preset::preset_dir();
                let _ = std::fs::create_dir_all(&dir);
                let path = dir.join(format!("{name}.ferric"));
                if let Err(err) = preset::save_preset(&path, &self.params) {
                    nih_plug::nih_warn!("preset save failed: {err}");
                    return;
                }
                let mut list = vec!["Init Patch".to_string()];
                list.extend(
                    preset::list_presets().iter().map(|p| preset::preset_name(p)),
                );
                let new_name = preset::preset_name(&path);
                let new_idx = list
                    .iter()
                    .position(|n| n == &new_name)
                    .unwrap_or(0);
                self.preset_state.list = list;
                self.preset_state.current_index = new_idx;
                self.preset_state.current_name = new_name;
                self.save_name.clear();
            }
        });
    }
}

fn build_initial_preset_state() -> PresetState {
    let mut list = vec!["Init Patch".to_string()];
    list.extend(preset::list_presets().iter().map(|p| preset::preset_name(p)));
    PresetState {
        list,
        current_index: 0,
        current_name: "Init Patch".to_string(),
    }
}

/// Build a fresh `ViziaState` at the requested scale. Called from
/// `DynamicScaleEditor::spawn` each time the GUI is opened so the scale
/// picked via the `gui_scale` parameter is baked into the size the host
/// queries.
pub fn make_state(scale: f64) -> Arc<ViziaState> {
    ViziaState::new_with_default_scale_factor(
        || (crate::DESIGN_WIDTH, crate::DESIGN_HEIGHT),
        scale,
    )
}

pub fn create(
    params: Arc<FerricParams>,
    editor_state: Arc<ViziaState>,
    shared_state: Arc<SharedState>,
) -> Option<Box<dyn Editor>> {
    create_vizia_editor(editor_state, ViziaTheming::Custom, move |cx, _| {
        assets::register_noto_sans_light(cx);
        assets::register_noto_sans_thin(cx);

        // CSS handles only colors / accents; layout is inline below so a
        // stylesheet parse glitch can't break the structure.
        let _ = cx.add_stylesheet(STYLE);

        AppData {
            params: params.clone(),
            shared: shared_state.clone(),
            preset_state: build_initial_preset_state(),
            save_name: String::new(),
            theme: Theme::Classic,
            show_settings: false,
        }
        .build(cx);

        let tape_state = shared_state.clone();
        let tape_params = params.clone();
        let scope_state = shared_state.clone();

        ZStack::new(cx, move |cx| {
        VStack::new(cx, move |cx| {
            // Header — logo on the left, preset browser + Save on the right.
            HStack::new(cx, |cx| {
                VStack::new(cx, |cx| {
                    LogoView::new(cx)
                        .class("logo")
                        .width(Stretch(1.0))
                        .height(Pixels(34.0));
                    // Subtitle — wide letter-spacing approximated with literal
                    // spaces (vizia in this version doesn't expose letter-
                    // spacing). Theme color comes from `.logo-subtitle`
                    // overrides in editor.css.
                    Label::new(cx, "S T E R E O   T A P E   D E L A Y")
                        .class("logo-subtitle")
                        .width(Stretch(1.0))
                        .height(Pixels(12.0))
                        .child_left(Stretch(1.0))
                        .child_right(Stretch(1.0));
                })
                .width(Pixels(220.0))
                .height(Pixels(46.0))
                .top(Stretch(1.0))
                .bottom(Stretch(1.0));
                Element::new(cx).width(Stretch(1.0));
                // Hand-rolled dropdown rather than vizia's `PickList`:
                // PickList wraps a `Dropdown` inside a `picklist` view and
                // both layers paint, producing visible double borders we
                // couldn't suppress reliably. This way there's only one
                // element to style.
                Dropdown::new(
                    cx,
                    |cx| {
                        HStack::new(cx, |cx| {
                            Label::new(
                                cx,
                                AppData::preset_state.then(PresetState::current_name),
                            )
                            .child_top(Stretch(1.0))
                            .child_bottom(Stretch(1.0));
                            Label::new(cx, "\u{25BE}")
                                .child_top(Stretch(1.0))
                                .child_bottom(Stretch(1.0))
                                .right(Pixels(2.0));
                        })
                        .col_between(Stretch(1.0))
                        .child_left(Pixels(8.0))
                        .child_right(Pixels(8.0))
                    },
                    |cx| {
                        List::new(
                            cx,
                            AppData::preset_state.then(PresetState::list),
                            |cx, idx, item| {
                                Label::new(cx, item)
                                    .on_press(move |ex| {
                                        ex.emit(PresetEvent::LoadByIdx(idx));
                                        ex.emit(PopupEvent::Close);
                                    })
                                    .width(Stretch(1.0))
                                    .height(Pixels(22.0))
                                    .child_left(Pixels(8.0))
                                    .child_top(Stretch(1.0))
                                    .child_bottom(Stretch(1.0));
                            },
                        );
                    },
                )
                .class("preset-dropdown")
                .width(Pixels(180.0))
                .height(Pixels(26.0))
                .top(Stretch(1.0))
                .bottom(Stretch(1.0));
                Textbox::new(cx, AppData::save_name)
                    .on_edit(|ex, s| ex.emit(PresetEvent::EditSaveName(s)))
                    .on_submit(|ex, _, _| ex.emit(PresetEvent::SaveAs))
                    .class("preset-name-input")
                    .width(Pixels(140.0))
                    .height(Pixels(26.0))
                    .top(Stretch(1.0))
                    .bottom(Stretch(1.0));
                Button::new(
                    cx,
                    |ex| ex.emit(PresetEvent::SaveAs),
                    |cx| Label::new(cx, "Save"),
                )
                .class("toggle")
                .class("preset-btn")
                .width(Pixels(60.0))
                .height(Pixels(26.0))
                .top(Stretch(1.0))
                .bottom(Stretch(1.0));
                // Three filled-circle dots in an HStack, drawn explicitly
                // rather than as the "⋯" glyph — the unicode character has
                // unpredictable centering metrics in vizia's text rendering.
                HStack::new(cx, |cx| {
                    Element::new(cx).class("settings-dot");
                    Element::new(cx).class("settings-dot");
                    Element::new(cx).class("settings-dot");
                })
                .on_press(|ex| ex.emit(SettingsEvent::Toggle))
                .class("settings-btn")
                .width(Pixels(28.0))
                .height(Pixels(26.0))
                .col_between(Pixels(3.0))
                .child_left(Stretch(1.0))
                .child_right(Stretch(1.0))
                .child_top(Stretch(1.0))
                .child_bottom(Stretch(1.0))
                .top(Stretch(1.0))
                .bottom(Stretch(1.0));
            })
            .width(Stretch(1.0))
            .height(Pixels(58.0))
            .col_between(Pixels(8.0));

            // Top row of three control panels.
            HStack::new(cx, |cx| {
                panel(cx, "DELAY", |cx| {
                    param_row(cx, "TIME", |p| &p.time_ms);
                    param_row(cx, "SYNC", |p| &p.sync);
                    param_row(cx, "FEEDBACK", |p| &p.feedback);
                    param_row(cx, "SPREAD", |p| &p.spread);
                });
                panel(cx, "TAPE", |cx| {
                    param_row(cx, "DRIVE", |p| &p.drive);
                    param_row(cx, "TONE", |p| &p.tone);
                    param_row(cx, "WOW", |p| &p.wow);
                    param_row(cx, "FLUTTER", |p| &p.flutter);
                });
                panel(cx, "MODE & MIX", |cx| {
                    param_row(cx, "MODE", |p| &p.mode);
                    param_row(cx, "MIX", |p| &p.mix);
                    param_row(cx, "INPUT", |p| &p.input_gain_db);
                    param_row(cx, "OUTPUT", |p| &p.output_gain_db);
                });
            })
            .width(Stretch(1.0))
            .height(Pixels(170.0))
            .col_between(Pixels(10.0));

            // Tape transport view — L/R read-head markers with echo ghosts.
            // Title omitted; the lanes make the panel's purpose obvious.
            VStack::new(cx, move |cx| {
                TapeView::new(cx, tape_state.clone(), tape_params.clone())
                    .class("tape-view")
                    .width(Stretch(1.0))
                    .height(Stretch(1.0));
            })
            .class("tape-panel")
            .width(Stretch(1.0))
            .height(Pixels(118.0))
            .child_space(Pixels(10.0));

            // Modulation panel: 2 LFOs (shape/sync/rate/depth/target).
            VStack::new(cx, |cx| {
                lfo_row(
                    cx,
                    "LFO 1",
                    |p| &p.lfo1_shape,
                    |p| &p.lfo1_sync,
                    |p| &p.lfo1_rate_hz,
                    |p| &p.lfo1_depth,
                    |p| &p.lfo1_target,
                );
                lfo_row(
                    cx,
                    "LFO 2",
                    |p| &p.lfo2_shape,
                    |p| &p.lfo2_sync,
                    |p| &p.lfo2_rate_hz,
                    |p| &p.lfo2_depth,
                    |p| &p.lfo2_target,
                );
            })
            .class("modulation-panel")
            .width(Stretch(1.0))
            .height(Pixels(78.0))
            .child_space(Pixels(10.0))
            .row_between(Pixels(10.0));

            // Realtime output scope — absorbs whatever vertical space is left.
            VStack::new(cx, move |cx| {
                Label::new(cx, "OUTPUT")
                    .class("panel-title")
                    .height(Pixels(14.0));
                ScopeView::new(cx, scope_state)
                    .class("scope-view")
                    .width(Stretch(1.0))
                    .height(Stretch(1.0));
            })
            .class("scope-panel")
            .width(Stretch(1.0))
            .height(Stretch(1.0))
            .child_space(Pixels(10.0))
            .row_between(Pixels(6.0));

            // Bottom toggle bar + GUI scale picker.
            HStack::new(cx, |cx| {
                ParamButton::new(cx, AppData::params, |p| &p.hold)
                    .with_label("HOLD")
                    .class("toggle")
                    .class("hold")
                    .width(Pixels(100.0))
                    .height(Stretch(1.0));
                ParamButton::new(cx, AppData::params, |p| &p.overdub)
                    .with_label("ADD")
                    .class("toggle")
                    .class("add")
                    .width(Pixels(100.0))
                    .height(Stretch(1.0));
                ParamButton::new(cx, AppData::params, |p| &p.reverse)
                    .with_label("REVERSE")
                    .class("toggle")
                    .class("reverse")
                    .width(Pixels(100.0))
                    .height(Stretch(1.0));
                // Spacer so SCALE sits flush right.
                Element::new(cx).width(Stretch(1.0));
                Label::new(cx, "SCALE")
                    .class("footer-text")
                    .width(Pixels(38.0))
                    .height(Stretch(1.0))
                    .child_top(Stretch(1.2))
                    .child_bottom(Stretch(1.0));
                ParamSlider::new(cx, AppData::params, |p| &p.gui_scale)
                    .class("subtle")
                    .width(Pixels(110.0))
                    .height(Pixels(22.0))
                    .top(Stretch(1.0))
                    .bottom(Stretch(1.0));
            })
            .width(Stretch(1.0))
            .height(Pixels(32.0))
            .col_between(Pixels(8.0));
        })
        .width(Stretch(1.0))
        .height(Stretch(1.0))
        .child_space(Pixels(14.0))
        .row_between(Pixels(10.0));

        // Modal backdrop — click outside the settings panel closes it.
        Element::new(cx)
            .class("settings-backdrop")
            .display(AppData::show_settings)
            .width(Stretch(1.0))
            .height(Stretch(1.0))
            .on_press(|ex| ex.emit(SettingsEvent::Toggle));

        // Settings panel — plugin info + theme picker.
        VStack::new(cx, |cx| {
            Label::new(cx, "FERRIC").class("app-title");
            Label::new(
                cx,
                concat!(
                    "Stereo Tape Delay  \u{00b7}  v",
                    env!("CARGO_PKG_VERSION")
                ),
            )
            .class("subtitle");
            Label::new(cx, "Realtime Media \u{00b7} GPL-3.0-or-later")
                .class("footer-text");
            Element::new(cx).height(Pixels(8.0));
            Label::new(
                cx,
                "Stereo tape delay inspired by the Erica Synths Black Stereo Delay:\ntape / digital / ping-pong modes, hold + add, reverse, wow & flutter,\ntempo sync and 2 LFOs with DAW sync.",
            )
            .class("footer-text");
            Element::new(cx).height(Pixels(12.0));
            Label::new(cx, "THEME").class("panel-title");
            HStack::new(cx, |cx| {
                Button::new(
                    cx,
                    |ex| ex.emit(SettingsEvent::SetTheme(Theme::Classic)),
                    |cx| Label::new(cx, "Classic"),
                )
                .class("toggle")
                .class("theme-pick")
                .checked(AppData::theme.map(|t| *t == Theme::Classic))
                .width(Stretch(1.0))
                .height(Stretch(1.0));
                Button::new(
                    cx,
                    |ex| ex.emit(SettingsEvent::SetTheme(Theme::Terminal)),
                    |cx| Label::new(cx, "Terminal"),
                )
                .class("toggle")
                .class("theme-pick")
                .checked(AppData::theme.map(|t| *t == Theme::Terminal))
                .width(Stretch(1.0))
                .height(Stretch(1.0));
            })
            .height(Pixels(30.0))
            .col_between(Pixels(8.0));
            Element::new(cx).height(Stretch(1.0));
            Button::new(
                cx,
                |ex| ex.emit(SettingsEvent::Toggle),
                |cx| Label::new(cx, "Close"),
            )
            .class("toggle")
            .width(Pixels(100.0))
            .height(Pixels(28.0))
            .left(Stretch(1.0));
        })
        .class("settings-overlay")
        .display(AppData::show_settings)
        .width(Pixels(380.0))
        .height(Pixels(300.0))
        .top(Stretch(1.0))
        .bottom(Stretch(1.0))
        .left(Stretch(1.0))
        .right(Stretch(1.0))
        .child_space(Pixels(20.0))
        .row_between(Pixels(6.0));
        })
        .class("root")
        .toggle_class("theme-terminal", AppData::theme.map(|t| *t == Theme::Terminal))
        .width(Stretch(1.0))
        .height(Stretch(1.0));

        // Drive a redraw at ~30 Hz so the TapeView markers and the scope
        // animate even when the user isn't interacting.
        let timer = cx.add_timer(
            Duration::from_millis(33),
            None,
            |cx, _action| {
                cx.needs_redraw();
            },
        );
        cx.start_timer(timer);
    })
}

fn panel<F: FnOnce(&mut Context) + 'static>(
    cx: &mut Context,
    title: &'static str,
    body: F,
) {
    VStack::new(cx, move |cx| {
        Label::new(cx, title)
            .class("panel-title")
            .height(Pixels(14.0));
        VStack::new(cx, body)
            .width(Stretch(1.0))
            .height(Stretch(1.0))
            .row_between(Pixels(10.0));
    })
    .class("panel")
    .width(Stretch(1.0))
    .height(Stretch(1.0))
    .child_space(Pixels(10.0))
    .row_between(Pixels(6.0));
}

fn param_row<F, P>(cx: &mut Context, label: &'static str, get: F)
where
    F: Fn(&Arc<FerricParams>) -> &P + Copy + 'static,
    P: Param + 'static,
{
    HStack::new(cx, move |cx| {
        Label::new(cx, label)
            .class("param-label")
            .width(Pixels(72.0))
            .child_top(Stretch(1.0))
            .child_bottom(Stretch(1.0));
        ParamSlider::new(cx, AppData::params, get).width(Stretch(1.0));
    })
    .width(Stretch(1.0))
    .height(Pixels(24.0))
    .col_between(Pixels(8.0));
}

fn lfo_row<S, Y, R, D, T, PS, PY, PR, PD, PT>(
    cx: &mut Context,
    label: &'static str,
    shape: S,
    sync: Y,
    rate: R,
    depth: D,
    target: T,
) where
    S: Fn(&Arc<FerricParams>) -> &PS + Copy + 'static,
    Y: Fn(&Arc<FerricParams>) -> &PY + Copy + 'static,
    R: Fn(&Arc<FerricParams>) -> &PR + Copy + 'static,
    D: Fn(&Arc<FerricParams>) -> &PD + Copy + 'static,
    T: Fn(&Arc<FerricParams>) -> &PT + Copy + 'static,
    PS: Param + 'static,
    PY: Param + 'static,
    PR: Param + 'static,
    PD: Param + 'static,
    PT: Param + 'static,
{
    HStack::new(cx, move |cx| {
        Label::new(cx, label)
            .class("param-label")
            .width(Pixels(50.0))
            .child_top(Stretch(1.0))
            .child_bottom(Stretch(1.0));
        ParamSlider::new(cx, AppData::params, shape).width(Stretch(1.0));
        ParamSlider::new(cx, AppData::params, sync).width(Stretch(1.0));
        ParamSlider::new(cx, AppData::params, rate).width(Stretch(1.0));
        ParamSlider::new(cx, AppData::params, depth).width(Stretch(1.0));
        ParamSlider::new(cx, AppData::params, target).width(Stretch(1.2));
    })
    .width(Stretch(1.0))
    .height(Pixels(24.0))
    .col_between(Pixels(6.0));
}

// --- Custom widget: tape transport / delay-tap display ---
//
// Two lanes (L / R). Each lane shows the write head at the left edge
// ("now"), the read tap at its current effective delay — including the
// tape slew and wow/flutter drift, so the marker physically wanders — and
// ghost markers at each feedback repeat, fading with the feedback amount.

pub struct TapeView {
    state: Arc<SharedState>,
    params: Arc<FerricParams>,
}

impl TapeView {
    pub fn new(
        cx: &mut Context,
        state: Arc<SharedState>,
        params: Arc<FerricParams>,
    ) -> Handle<'_, Self> {
        Self { state, params }.build(cx, |_| {})
    }
}

/// Window shown by the lane x-axis, in ms. A bit past the max delay time
/// so a full-length tap doesn't sit exactly on the right edge.
const LANE_WINDOW_MS: f32 = 3300.0;

impl View for TapeView {
    fn element(&self) -> Option<&'static str> {
        Some("tape-view")
    }

    fn draw(&self, cx: &mut DrawContext, canvas: &mut Canvas) {
        let bounds = cx.bounds();
        if bounds.w <= 0.0 || bounds.h <= 0.0 {
            return;
        }

        let is_terminal =
            self.state.theme.load(std::sync::atomic::Ordering::Relaxed) == 1;
        let (delay_l, delay_r) = self.state.load_delay_ms();
        let feedback = self.params.feedback.value().clamp(0.0, 1.1);
        let hold = self.params.hold.value();
        let reverse = self.params.reverse.value();

        let gap = 6.0;
        let lane_h = (bounds.h - gap) / 2.0;

        let accent = if is_terminal {
            VgColor::rgbf(0.45, 1.00, 0.65)
        } else {
            VgColor::rgbf(0.40, 0.70, 0.95)
        };
        let hold_color = VgColor::rgbf(0.37, 0.95, 0.78);
        let tap_color = if hold { hold_color } else { accent };
        let grid_color = if is_terminal {
            VgColor::rgbaf(0.10, 0.50, 0.20, 0.45)
        } else {
            VgColor::rgbaf(0.25, 0.40, 0.60, 0.40)
        };

        for (lane, delay_ms) in [(0usize, delay_l), (1usize, delay_r)] {
            let ly = bounds.y + lane as f32 * (lane_h + gap);
            let lx = bounds.x;
            let lw = bounds.w;

            // Lane background.
            let mut bg = Path::new();
            bg.rounded_rect(lx, ly, lw, lane_h, 4.0);
            canvas.fill_path(&bg, &Paint::color(VgColor::rgbf(0.0, 0.0, 0.0)));

            // Time ruler — a tick every 500 ms.
            let mut t = 500.0;
            while t < LANE_WINDOW_MS {
                let x = lx + 2.0 + (t / LANE_WINDOW_MS) * (lw - 4.0);
                let mut tick = Path::new();
                tick.move_to(x, ly + 2.0);
                tick.line_to(x, ly + lane_h - 2.0);
                canvas.stroke_path(&tick, &Paint::color(grid_color).with_line_width(1.0));
                t += 500.0;
            }

            // Center line.
            let cy = ly + lane_h / 2.0;
            let mut center = Path::new();
            center.move_to(lx + 2.0, cy);
            center.line_to(lx + lw - 2.0, cy);
            canvas.stroke_path(
                &center,
                &Paint::color(grid_color).with_line_width(1.0),
            );

            // Write head — fixed at the left edge.
            let mut wh = Path::new();
            wh.rounded_rect(lx + 3.0, ly + 3.0, 3.0, lane_h - 6.0, 1.5);
            canvas.fill_path(
                &wh,
                &Paint::color(VgColor::rgbaf(1.0, 1.0, 1.0, 0.85)),
            );

            if !delay_ms.is_finite() || delay_ms <= 0.0 {
                continue;
            }

            // Read tap + feedback ghosts. Repeat k sits at (k+1)·delay with
            // brightness feedback^k; hold pins every repeat at full level.
            let mut k = 0u32;
            loop {
                let t_ms = delay_ms * (k + 1) as f32;
                if t_ms > LANE_WINDOW_MS || k > 24 {
                    break;
                }
                let alpha = if hold {
                    0.9
                } else if k == 0 {
                    0.9
                } else {
                    0.9 * feedback.min(1.0).powi(k as i32)
                };
                if alpha < 0.05 {
                    break;
                }
                let x = lx + 2.0 + (t_ms / LANE_WINDOW_MS) * (lw - 4.0);
                let bar_h = (lane_h - 8.0) * (0.35 + 0.65 * alpha);
                let by = cy - bar_h / 2.0;

                // Soft glow behind the primary tap.
                if k == 0 {
                    let mut glow = Path::new();
                    glow.rounded_rect(x - 4.0, by - 2.0, 9.0, bar_h + 4.0, 4.0);
                    canvas.fill_path(
                        &glow,
                        &Paint::color(VgColor::rgbaf(
                            tap_color.r, tap_color.g, tap_color.b, 0.18,
                        )),
                    );
                }
                let mut bar = Path::new();
                bar.rounded_rect(x - 1.5, by, 3.0, bar_h, 1.5);
                canvas.fill_path(
                    &bar,
                    &Paint::color(VgColor::rgbaf(
                        tap_color.r, tap_color.g, tap_color.b, alpha,
                    )),
                );

                // Reverse — a small left-pointing wedge on the primary tap.
                if reverse && k == 0 {
                    let mut tri = Path::new();
                    tri.move_to(x - 3.0, ly + 7.0);
                    tri.line_to(x - 10.0, ly + 11.0);
                    tri.line_to(x - 3.0, ly + 15.0);
                    tri.close();
                    canvas.fill_path(
                        &tri,
                        &Paint::color(VgColor::rgbaf(
                            tap_color.r, tap_color.g, tap_color.b, 0.9,
                        )),
                    );
                }

                k += 1;
            }
        }
    }
}

// --- Custom widget: FERRIC logo (vector). ---
//
// Icon mark: two tape reels on a tape line. Wordmark: "FERRIC" letterforms
// drawn as stroked paths in the same squared style as the STASIS mark, so
// the logo stays crisp across the 0.75×..2× GUI scale range.

pub struct LogoView;

impl LogoView {
    pub fn new(cx: &mut Context) -> Handle<'_, Self> {
        Self.build(cx, |_| {})
    }
}

impl View for LogoView {
    fn element(&self) -> Option<&'static str> {
        Some("logo-view")
    }

    fn draw(&self, cx: &mut DrawContext, canvas: &mut Canvas) {
        let bounds = cx.bounds();
        if bounds.w <= 0.0 || bounds.h <= 0.0 {
            return;
        }

        // Aspect-fit the source artboard into the widget bounds.
        const SVG_W: f32 = 500.0;
        const SVG_H: f32 = 75.0;
        let scale = (bounds.w / SVG_W).min(bounds.h / SVG_H);
        let off_x = bounds.x + (bounds.w - SVG_W * scale) / 2.0;
        let off_y = bounds.y + (bounds.h - SVG_H * scale) / 2.0;
        let p = |x: f32, y: f32| (x * scale + off_x, y * scale + off_y);

        let white = VgColor::rgbf(1.0, 1.0, 1.0);
        let dot_grey = VgColor::rgbf(0.647, 0.647, 0.647);

        let stroke_w = (3.2 * scale).max(1.0);
        let paint = Paint::color(white)
            .with_line_width(stroke_w)
            .with_line_cap(femtovg_line_cap_square())
            .with_line_join(femtovg_line_join_round());

        // --- Icon: two tape reels + tape line. ---
        // Reels centered at y=32, r=26 — top rail at y=6, bottom tangent at
        // y=58, matching the wordmark's vertical extent.
        for (cx_, alpha) in [(35.0_f32, 1.0_f32), (120.0, 0.8)] {
            let (rx, ry) = p(cx_, 32.0);
            let reel_paint = Paint::color(VgColor::rgbaf(1.0, 1.0, 1.0, alpha))
                .with_line_width(stroke_w);
            let mut reel = Path::new();
            reel.circle(rx, ry, 26.0 * scale);
            canvas.stroke_path(&reel, &reel_paint);
            // Hub.
            let mut hub = Path::new();
            hub.circle(rx, ry, 5.0 * scale);
            canvas.fill_path(&hub, &Paint::color(VgColor::rgbaf(1.0, 1.0, 1.0, alpha)));
            // Three spoke dots.
            for i in 0..3 {
                let a = i as f32 * std::f32::consts::TAU / 3.0
                    - std::f32::consts::FRAC_PI_2;
                let sx = rx + a.cos() * 15.0 * scale;
                let sy = ry + a.sin() * 15.0 * scale;
                let mut spoke = Path::new();
                spoke.circle(sx, sy, 2.6 * scale);
                canvas.fill_path(
                    &spoke,
                    &Paint::color(VgColor::rgbaf(1.0, 1.0, 1.0, alpha * 0.7)),
                );
            }
        }
        // Tape running under both reels.
        let mut tape = Path::new();
        let (tx0, ty) = p(12.0, 58.0);
        let (tx1, _) = p(146.0, 58.0);
        tape.move_to(tx0, ty);
        tape.line_to(tx1, ty);
        canvas.stroke_path(
            &tape,
            &Paint::color(VgColor::rgbaf(1.0, 1.0, 1.0, 0.6))
                .with_line_width((2.4 * scale).max(1.0)),
        );

        // --- Wordmark: FERRIC. Letters live in y=6.8..62.8 like STASIS. ---
        let bez = |path: &mut Path, c1: (f32, f32), c2: (f32, f32), end: (f32, f32)| {
            let (cx1, cy1) = p(c1.0, c1.1);
            let (cx2, cy2) = p(c2.0, c2.1);
            let (ex, ey) = p(end.0, end.1);
            path.bezier_to(cx1, cy1, cx2, cy2, ex, ey);
        };
        let line = |path: &mut Path, end: (f32, f32)| {
            let (ex, ey) = p(end.0, end.1);
            path.line_to(ex, ey);
        };
        let stroke_from = |canvas: &mut Canvas, start: (f32, f32), f: &dyn Fn(&mut Path)| {
            let mut path = Path::new();
            let (sx, sy) = p(start.0, start.1);
            path.move_to(sx, sy);
            f(&mut path);
            canvas.stroke_path(&path, &paint);
        };

        // "F" — stem + top bar + mid bar.
        let x = 173.0;
        stroke_from(canvas, (x, 6.8), &|pa| line(pa, (x, 62.8)));
        stroke_from(canvas, (x, 6.8), &|pa| line(pa, (x + 44.0, 6.8)));
        stroke_from(canvas, (x, 34.8), &|pa| line(pa, (x + 36.0, 34.8)));

        // "E" — stem + three bars.
        let x = 236.0;
        stroke_from(canvas, (x, 6.8), &|pa| line(pa, (x, 62.8)));
        stroke_from(canvas, (x, 6.8), &|pa| line(pa, (x + 44.0, 6.8)));
        stroke_from(canvas, (x, 34.8), &|pa| line(pa, (x + 36.0, 34.8)));
        stroke_from(canvas, (x, 62.8), &|pa| line(pa, (x + 44.0, 62.8)));

        // "R" ×2 — stem + rounded bowl + leg.
        for x in [299.0_f32, 362.0] {
            stroke_from(canvas, (x, 6.8), &|pa| line(pa, (x, 62.8)));
            stroke_from(canvas, (x, 6.8), &|pa| {
                line(pa, (x + 40.0, 6.8));
                bez(pa, (x + 45.33, 6.8), (x + 48.0, 9.47), (x + 48.0, 14.8));
                line(pa, (x + 48.0, 26.8));
                bez(pa, (x + 48.0, 32.13), (x + 45.33, 34.8), (x + 40.0, 34.8));
                line(pa, (x, 34.8));
            });
            stroke_from(canvas, (x + 26.0, 34.8), &|pa| line(pa, (x + 48.0, 62.8)));
        }

        // "I" — stem + accent dot below (the brand's signature detail).
        let x = 429.0;
        stroke_from(canvas, (x, 6.8), &|pa| line(pa, (x, 62.8)));
        let (dx, dy) = p(x, 69.8);
        let mut dot = Path::new();
        dot.circle(dx, dy, 2.2 * scale);
        canvas.fill_path(&dot, &Paint::color(dot_grey));

        // "C" — top bar, rounded left corners, bottom bar.
        let x = 448.0;
        stroke_from(canvas, (x + 48.0, 6.8), &|pa| {
            line(pa, (x + 8.0, 6.8));
            bez(pa, (x + 2.67, 6.8), (x, 9.47), (x, 14.8));
            line(pa, (x, 54.8));
            bez(pa, (x, 60.13), (x + 2.67, 62.8), (x + 8.0, 62.8));
            line(pa, (x + 48.0, 62.8));
        });
    }
}

fn femtovg_line_cap_square() -> nih_plug_vizia::vizia::vg::LineCap {
    nih_plug_vizia::vizia::vg::LineCap::Square
}

fn femtovg_line_join_round() -> nih_plug_vizia::vizia::vg::LineJoin {
    nih_plug_vizia::vizia::vg::LineJoin::Round
}

// --- Custom widget: realtime output oscilloscope ---

pub struct ScopeView {
    state: Arc<SharedState>,
}

impl ScopeView {
    pub fn new(cx: &mut Context, state: Arc<SharedState>) -> Handle<'_, Self> {
        Self { state }.build(cx, |_| {})
    }
}

impl View for ScopeView {
    fn element(&self) -> Option<&'static str> {
        Some("scope-view")
    }

    fn draw(&self, cx: &mut DrawContext, canvas: &mut Canvas) {
        let bounds = cx.bounds();
        if bounds.w <= 0.0 || bounds.h <= 0.0 {
            return;
        }

        // Force the black background here rather than relying on the CSS
        // `scope-view { background-color: #000000 }` rule — under
        // `ViziaTheming::Custom` element selectors don't always cascade
        // reliably, and we *always* want the scope on a black canvas.
        let mut bg = Path::new();
        bg.rounded_rect(bounds.x, bounds.y, bounds.w, bounds.h, 4.0);
        canvas.fill_path(&bg, &Paint::color(VgColor::rgbf(0.0, 0.0, 0.0)));

        let scope_size = crate::state::SCOPE_SIZE;
        let write = self.state.scope_write_pos();
        let cy = bounds.y + bounds.h / 2.0;
        let max_amp = (bounds.h / 2.0 - 4.0).max(1.0);

        // Auto-scale to the loudest sample in the visible window so quiet
        // mix-bus levels fill the display. Floor at 0.05 keeps near-silence
        // from blowing up the noise floor; cap at 1.0 means we never
        // compress hot signals.
        let mut peak = 0.0_f32;
        for i in 0..scope_size {
            let s = self.state.scope_load_at(i).abs();
            if s > peak {
                peak = s;
            }
        }
        let scale = 1.0 / peak.max(0.05).min(1.0);

        // CRT-style phosphor graticule behind the trace. 10 vertical and
        // 8 horizontal divisions, full-pixel lines so they don't disappear
        // at sub-pixel widths.
        let grid_paint = Paint::color(VgColor::rgbaf(0.10, 0.50, 0.20, 0.45))
            .with_line_width(1.0);
        let v_div = 10;
        for i in 1..v_div {
            let x = bounds.x + (i as f32 / v_div as f32) * bounds.w;
            let mut line = Path::new();
            line.move_to(x, bounds.y + 2.0);
            line.line_to(x, bounds.y + bounds.h - 2.0);
            canvas.stroke_path(&line, &grid_paint);
        }
        let h_div = 8;
        for i in 1..h_div {
            // Skip the center row — the brighter axis line below renders there.
            if i == h_div / 2 {
                continue;
            }
            let y = bounds.y + (i as f32 / h_div as f32) * bounds.h;
            let mut line = Path::new();
            line.move_to(bounds.x + 2.0, y);
            line.line_to(bounds.x + bounds.w - 2.0, y);
            canvas.stroke_path(&line, &grid_paint);
        }

        // Center axis line — brighter than the rest of the grid so the
        // zero crossing reads at a glance.
        let mut axis = Path::new();
        axis.move_to(bounds.x + 2.0, cy);
        axis.line_to(bounds.x + bounds.w - 2.0, cy);
        canvas.stroke_path(
            &axis,
            &Paint::color(VgColor::rgbaf(0.15, 0.70, 0.30, 0.65)).with_line_width(1.2),
        );

        // Waveform line — oldest sample on the left, newest on the right.
        let mut wave = Path::new();
        let plot_w = bounds.w - 4.0;
        let plot_x = bounds.x + 2.0;
        for i in 0..scope_size {
            let idx = (write + i) % scope_size;
            let s = self.state.scope_load_at(idx);
            let x = plot_x + (i as f32 / scope_size as f32) * plot_w;
            let y = cy - (s * scale).clamp(-1.0, 1.0) * max_amp;
            if i == 0 {
                wave.move_to(x, y);
            } else {
                wave.line_to(x, y);
            }
        }
        let is_terminal =
            self.state.theme.load(std::sync::atomic::Ordering::Relaxed) == 1;
        let trace_color = if is_terminal {
            VgColor::rgbf(0.45, 1.00, 0.55)
        } else {
            VgColor::rgbf(1.00, 1.00, 1.00)
        };
        canvas.stroke_path(&wave, &Paint::color(trace_color).with_line_width(1.2));
    }
}
