use egui::{Color32, Context, Event, Key, Pos2, RawInput, Rect, Role, Sense, Stroke, WidgetInfo};
use std::collections::BTreeMap;

#[derive(Clone)]
struct Note {
    id: u64,
    tick: u64,
    duration: u64,
    pitch: u8,
    velocity: u16,
}

#[derive(Default)]
struct PerformanceKeys {
    held: BTreeMap<Key, u8>,
    transitions: Vec<(u8, bool)>,
}

impl PerformanceKeys {
    // Releases survive text acquiring focus. Device MIDI belongs to the engine.
    fn route(&mut self, events: &[Event], text_focused: bool) {
        for event in events {
            match event {
                Event::WindowFocused(false) => {
                    for (_, pitch) in std::mem::take(&mut self.held) {
                        self.transitions.push((pitch, false));
                    }
                }
                Event::Key {
                    key,
                    physical_key,
                    pressed,
                    repeat,
                    modifiers,
                } => {
                    let physical = physical_key.unwrap_or(*key);
                    if !pressed {
                        if let Some(pitch) = self.held.remove(&physical) {
                            self.transitions.push((pitch, false));
                        }
                    } else if !text_focused
                        && !repeat
                        && !modifiers.command
                        && !modifiers.alt
                        && let Some(pitch) = match physical {
                            Key::A => Some(60),
                            Key::S => Some(62),
                            _ => None,
                        }
                        && self.held.insert(physical, pitch).is_none()
                    {
                        self.transitions.push((pitch, true));
                    }
                }
                _ => {}
            }
        }
    }
}

struct StudioProof {
    notes: Vec<Note>,
    selected: Option<u64>,
    visible_rects: Vec<(u64, Rect)>,
    title: String,
    title_id: egui::Id,
    keys: PerformanceKeys,
}

impl Default for StudioProof {
    fn default() -> Self {
        Self {
            notes: vec![Note {
                id: 1,
                tick: 123,
                duration: 697,
                pitch: 60,
                velocity: 54321,
            }],
            selected: None,
            visible_rects: vec![],
            title: "Untitled".into(),
            title_id: egui::Id::unique("project-title"),
            keys: PerformanceKeys::default(),
        }
    }
}

impl StudioProof {
    fn frame(&mut self, ctx: &Context, events: Vec<Event>) -> egui::FullOutput {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1100.0, 700.0))),
            events,
            ..Default::default()
        };
        self.visible_rects.clear();
        let mut output =
            ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.heading("OmniuSong native UI proof");
                    ui.horizontal(|ui| {
                        let _ = ui.button("Play");
                        let _ = ui.button("Stop");
                        let _ = ui.button("All notes off");
                    });
                    ui.name_inputs("Project title", |ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.title).id(self.title_id));
                    });
                    self.keys.route(
                        &ctx.input(|i| i.events.clone()),
                        ctx.egui_wants_keyboard_input(),
                    );
                    let (roll, _) =
                        ui.allocate_exact_size(egui::vec2(1020.0, 300.0), Sense::hover());
                    let painter = ui.painter().with_clip_rect(roll);
                    painter.rect_filled(roll, 0.0, Color32::from_rgb(20, 24, 31));
                    for beat in 0..=16 {
                        let x = roll.left() + beat as f32 * roll.width() / 16.0;
                        painter.line_segment(
                            [egui::pos2(x, roll.top()), egui::pos2(x, roll.bottom())],
                            Stroke::new(1.0, Color32::DARK_GRAY),
                        );
                    }
                    // Source ticks/velocities never become screen floats in storage.
                    for note in self.notes.iter().filter(|n| {
                        n.tick < 15360 && n.tick + n.duration > 0 && (48..=84).contains(&n.pitch)
                    }) {
                        let x = roll.left() + note.tick as f32 / 15360.0 * roll.width();
                        let y = roll.top() + (84 - note.pitch) as f32 / 37.0 * roll.height();
                        let rect = Rect::from_min_size(
                            egui::pos2(x, y),
                            egui::vec2(
                                (note.duration as f32 / 15360.0 * roll.width()).max(4.0),
                                7.0,
                            ),
                        );
                        let response =
                            ui.interact(rect, egui::Id::unique(("note", note.id)), Sense::click());
                        response.widget_info(|| {
                            WidgetInfo::selected(
                                Role::Button,
                                true,
                                self.selected == Some(note.id),
                                format!(
                                    "Note {} pitch {} tick {} duration {} velocity {}",
                                    note.id, note.pitch, note.tick, note.duration, note.velocity
                                ),
                            )
                        });
                        if response.clicked() {
                            self.selected = Some(note.id);
                        }
                        painter.rect_filled(rect, 2.0, Color32::from_rgb(83, 210, 178));
                        self.visible_rects.push((note.id, rect));
                    }
                    let (wave, response) =
                        ui.allocate_exact_size(egui::vec2(1020.0, 90.0), Sense::hover());
                    response.widget_info(|| WidgetInfo::labeled(Role::Image, true,
                    "Original audio waveform, 2 seconds, selected source 0.25 to 1.75 seconds"));
                    let painter = ui.painter().with_clip_rect(wave);
                    for bin in 0..256 {
                        let x = wave.left() + bin as f32 / 256.0 * wave.width();
                        let amplitude =
                            ((bin as f32 * 0.31).sin().abs() * 0.8 + 0.1) * wave.height() * 0.5;
                        painter.line_segment(
                            [
                                egui::pos2(x, wave.center().y - amplitude),
                                egui::pos2(x, wave.center().y + amplitude),
                            ],
                            Stroke::new(1.0, Color32::LIGHT_BLUE),
                        );
                    }
                    ui.horizontal(|ui| {
                        for pad in 1..=16 {
                            let _ = ui.button(format!("Pad {pad}"));
                        }
                    });
                });
            });
        // This proof has no GPU texture owner. A real eframe app uploads these.
        output.textures_delta.clear();
        output
    }
}

fn main() {
    let ctx = Context::default();
    ctx.enable_accesskit();
    let mut studio = StudioProof::default();
    let output = studio.frame(&ctx, vec![]);
    let nodes = output
        .platform_output
        .accesskit_update
        .as_ref()
        .map_or(0, |tree| tree.nodes.len());
    let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
    println!(
        "egui 0.37.0 headless proof: {} visible note, {nodes} accessibility nodes, {} painted primitives",
        studio.visible_rects.len(),
        primitives.len()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(key: Key, pressed: bool, repeat: bool) -> Event {
        Event::Key {
            key,
            physical_key: Some(key),
            pressed,
            repeat,
            modifiers: Default::default(),
        }
    }

    #[test]
    fn painted_content_has_accessible_actions_and_exact_note_description() {
        let ctx = Context::default();
        ctx.enable_accesskit();
        let output = StudioProof::default().frame(&ctx, vec![]);
        let nodes = output.platform_output.accesskit_update.unwrap().nodes;
        for expected in [
            "Play",
            "Stop",
            "All notes off",
            "Pad 16",
            "Note 1 pitch 60 tick 123 duration 697 velocity 54321",
            "Original audio waveform, 2 seconds, selected source 0.25 to 1.75 seconds",
        ] {
            assert!(
                nodes.iter().any(|(_, n)| n.label() == Some(expected)),
                "Missing {expected}"
            );
        }
        let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
        assert!(primitives.iter().any(
            |p| matches!(&p.primitive, egui::epaint::Primitive::Mesh(m) if !m.indices.is_empty())
        ));
    }

    #[test]
    fn pointer_hit_selects_note_without_quantizing_human_values() {
        let ctx = Context::default();
        let mut studio = StudioProof::default();
        let _ = studio.frame(&ctx, vec![]);
        let pos = studio.visible_rects[0].1.center();
        for pressed in [true, false] {
            let _ = studio.frame(
                &ctx,
                vec![
                    Event::PointerMoved(pos),
                    Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        assert_eq!(studio.selected, Some(1));
        assert_eq!(
            (
                studio.notes[0].tick,
                studio.notes[0].duration,
                studio.notes[0].velocity
            ),
            (123, 697, 54321)
        );
    }

    #[test]
    fn assistive_click_selects_the_same_custom_note_as_pointer_input() {
        let ctx = Context::default();
        ctx.enable_accesskit();
        let mut studio = StudioProof::default();
        let tree = studio
            .frame(&ctx, vec![])
            .platform_output
            .accesskit_update
            .unwrap();
        let target = tree
            .nodes
            .iter()
            .find(|(_, n)| n.label().is_some_and(|s| s.starts_with("Note 1 ")))
            .unwrap()
            .0;
        let _ = studio.frame(
            &ctx,
            vec![Event::AccessKitActionRequest(
                egui::accesskit::ActionRequest {
                    action: egui::accesskit::Action::Click,
                    target_tree: tree.tree_id,
                    target_node: target,
                    data: None,
                },
            )],
        );
        assert_eq!(studio.selected, Some(1));
    }

    #[test]
    fn offscreen_score_is_not_submitted_to_renderer_or_accessibility_tree() {
        let ctx = Context::default();
        ctx.enable_accesskit();
        let mut studio = StudioProof::default();
        studio.notes.extend((2..100_002).map(|id| Note {
            id,
            tick: 15360 + id * 960,
            duration: 697,
            pitch: 60,
            velocity: 54321,
        }));
        let output = studio.frame(&ctx, vec![]);
        assert_eq!(studio.visible_rects.len(), 1);
        let tree = output.platform_output.accesskit_update.unwrap();
        assert_eq!(
            tree.nodes
                .iter()
                .filter(|(_, n)| n.label().is_some_and(|s| s.starts_with("Note ")))
                .count(),
            1
        );
    }

    #[test]
    fn key_repeat_is_ignored_and_key_release_survives_new_text_focus() {
        let ctx = Context::default();
        let mut studio = StudioProof::default();
        let _ = studio.frame(
            &ctx,
            vec![key(Key::A, true, false), key(Key::A, true, true)],
        );
        ctx.memory_mut(|m| m.request_focus(studio.title_id));
        let _ = studio.frame(
            &ctx,
            vec![key(Key::A, false, false), key(Key::S, true, false)],
        );
        assert_eq!(studio.keys.transitions, vec![(60, true), (60, false)]);
        assert!(studio.keys.held.is_empty());
    }

    #[test]
    fn focus_loss_releases_all_computer_keys_once() {
        let ctx = Context::default();
        let mut studio = StudioProof::default();
        let _ = studio.frame(
            &ctx,
            vec![key(Key::A, true, false), key(Key::S, true, false)],
        );
        let _ = studio.frame(&ctx, vec![Event::WindowFocused(false)]);
        let _ = studio.frame(&ctx, vec![Event::WindowFocused(false)]);
        assert!(studio.keys.held.is_empty());
        assert_eq!(
            studio.keys.transitions,
            vec![(60, true), (62, true), (60, false), (62, false)]
        );
    }

    #[test]
    fn musical_keys_follow_physical_position_and_do_not_steal_command_shortcuts() {
        let mut router = PerformanceKeys::default();
        router.route(
            &[Event::Key {
                key: Key::Q,
                physical_key: Some(Key::A),
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
            false,
        );
        router.route(
            &[Event::Key {
                key: Key::S,
                physical_key: Some(Key::S),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers {
                    command: true,
                    ..Default::default()
                },
            }],
            false,
        );
        assert_eq!(router.transitions, vec![(60, true)]);
    }
}
