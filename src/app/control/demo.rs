//! UI-thread adapter and presentation overlay for the transport-independent player.
use super::*;
use crate::demo::{Effect, Player};

impl App {
    pub(super) fn control_start_demo(
        &mut self,
        script: crate::demo::Script,
    ) -> std::result::Result<(), Error> {
        self.control_idle()?;
        if self.control.demo.as_ref().is_some_and(Player::running) {
            return Err(Error::new(ErrorCode::Busy, "A demo is already running"));
        }
        let player = Player::new(script.clone(), &self.control_state())?;
        if let Some([x, y]) = script.window {
            self.ctx
                .send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(x, y)));
        }
        self.control.demo = Some(player);
        Ok(())
    }

    pub(in crate::app) fn tick_demo(&mut self) {
        let Some(mut player) = self.control.demo.take() else {
            return;
        };
        let manual_input = self.ctx.input(|i| {
            i.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::Key { pressed: true, .. }
                        | egui::Event::PointerButton { pressed: true, .. }
                        | egui::Event::MouseWheel { .. }
                        | egui::Event::Touch { .. }
                        | egui::Event::Text(_)
                        | egui::Event::Paste(_)
                )
            })
        });
        if manual_input {
            player.cancel();
        }
        let now = self.ctx.input(|i| i.time);
        match player.next(now, &self.control_state()) {
            Some(Effect::Command(command)) => {
                let poll = matches!(command, api::Command::PollOperation { .. });
                let target = if poll { None } else { player.target() };
                let result = self.execute_control(api::Request { command, target });
                if poll {
                    match result {
                        Ok(response) => player.observe_operation(response),
                        Err(error) => player.fail(error),
                    }
                } else {
                    player.accept(result, now);
                }
            }
            Some(Effect::View { zoom, scroll }) => {
                self.zoom_mode = ZoomMode::Custom;
                self.fit_requested = false;
                self.change_zoom(zoom, None);
                self.zoom_anchor = None;
                self.scroll_x = Some(scroll[0]);
                self.scroll_y = Some(scroll[1]);
            }
            None => {}
        }
        if player.running() {
            let painter = self.ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("demo-cues"),
            ));
            let rect = self.viewer_rect;
            if let Some([x, y]) = player.cue.pointer {
                let at = rect.min + rect.size() * vec2(x, y);
                painter.circle_filled(
                    at,
                    7.0,
                    egui::Color32::from_rgba_unmultiplied(255, 160, 40, 200),
                );
                painter.circle_stroke(at, 12.0, egui::Stroke::new(2.0, egui::Color32::WHITE));
            }
            let caption = match &player.cue.shortcut {
                Some(shortcut) => format!("{}   [{}]", player.cue.caption, shortcut),
                None => player.cue.caption.clone(),
            };
            if !caption.is_empty() {
                let galley = painter.layout_no_wrap(
                    caption,
                    egui::FontId::proportional(20.0),
                    egui::Color32::WHITE,
                );
                let at = pos2(
                    rect.center().x - galley.size().x / 2.0,
                    rect.bottom() - 64.0,
                );
                painter.rect_filled(
                    egui::Rect::from_min_size(at, galley.size()).expand(10.0),
                    6.0,
                    egui::Color32::from_black_alpha(210),
                );
                painter.galley(at, galley, egui::Color32::WHITE);
            }
            self.ctx.request_repaint_after(Duration::from_millis(16));
        }
        self.control.demo = Some(player);
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::tests::app_with_a_document;
    use super::*;
    use crate::demo::{Script, Status, Step};
    fn script(steps: Vec<Step>) -> Script {
        Script {
            version: 1,
            window: None,
            steps,
        }
    }

    #[test]
    fn example_script_is_valid_and_app_adapter_reuses_navigation() {
        let example: Script =
            serde_json::from_str(include_str!("../../../examples/demo/navigation.json")).unwrap();
        example.validate().unwrap();
        let (mut app, _) = app_with_a_document();
        app.control_start_demo(script(vec![
            Step::Command {
                command: api::Command::GoToPage { page: 3 },
            },
            Step::Command {
                command: api::Command::Invoke {
                    action: api::Action::QuickAccess,
                },
            },
        ]))
        .unwrap();
        app.tick_demo();
        assert_eq!(app.current_page, 2);
        app.tick_demo();
        assert!(app.palette.open);
        app.tick_demo();
        assert_eq!(
            app.control.demo.as_ref().unwrap().progress.status,
            Status::Complete
        );
    }

    #[test]
    fn player_waits_for_sharp_frames_and_animates_with_a_deterministic_clock() {
        let (app, _) = app_with_a_document();
        let mut state = app.control_state();
        state.zoom = 1.0;
        state.scroll = [10.0, 20.0];
        state.view_ready = false;
        let mut player = Player::new(
            script(vec![
                Step::WaitView { timeout_ms: 5000 },
                Step::Animate {
                    zoom: Some(2.0),
                    pan: [100.0, 40.0],
                    ms: 1000,
                },
            ]),
            &state,
        )
        .unwrap();
        assert!(player.next(0.0, &state).is_none());
        assert!(player.next(2.0, &state).is_none());
        assert_eq!(player.progress.step, 0);
        state.view_ready = true;
        player.next(2.1, &state);
        player.next(2.2, &state);
        assert_eq!(player.progress.step, 1);
        player.next(3.0, &state);
        let Some(Effect::View { zoom, scroll }) = player.next(3.5, &state) else {
            panic!("animation missing")
        };
        assert_eq!(zoom, 1.5);
        assert_eq!(scroll, [60.0, 40.0]);
        let Some(Effect::View { zoom, scroll }) = player.next(4.0, &state) else {
            panic!("animation missing")
        };
        assert_eq!(zoom, 2.0);
        assert_eq!(scroll, [110.0, 60.0]);
        player.next(4.1, &state);
        assert_eq!(player.progress.status, Status::Complete);
    }

    #[test]
    fn completion_is_worker_acknowledged_and_external_edits_stop_playback() {
        let (mut app, _) = app_with_a_document();
        let state = app.control_state();
        let mut player = Player::new(
            script(vec![
                Step::Command {
                    command: api::Command::Find {
                        query: "wall".into(),
                    },
                },
                Step::Pause { ms: 100 },
            ]),
            &state,
        )
        .unwrap();
        let Some(Effect::Command(_)) = player.next(0.0, &state) else {
            panic!()
        };
        let response = api::Response {
            state: state.clone(),
            data: None,
            operation: Some(api::Operation {
                id: 42,
                status: api::OperationStatus::Pending,
                error: None,
            }),
        };
        player.accept(Ok(response.clone()), 0.0);
        assert!(matches!(
            player.next(1.0, &state),
            Some(Effect::Command(api::Command::PollOperation { id: 42 }))
        ));
        player.observe_operation(response.clone());
        assert_eq!(player.progress.step, 0);
        let mut response = response;
        response.operation.as_mut().unwrap().status = api::OperationStatus::Complete;
        player.observe_operation(response);
        assert_eq!(player.progress.step, 1);
        app.doc.as_mut().unwrap().generation += 1;
        player.next(2.0, &app.control_state());
        assert_eq!(player.progress.status, Status::Failed);
        assert_eq!(
            player.progress.error.unwrap().code,
            ErrorCode::StaleDocument
        );
    }

    #[test]
    fn manual_input_cancels_before_the_next_command_and_nested_scripts_are_rejected() {
        let (mut app, _) = app_with_a_document();
        app.control_start_demo(script(vec![Step::Command {
            command: api::Command::GoToPage { page: 3 },
        }]))
        .unwrap();
        let ctx = app.ctx.clone();
        let mut output = ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |_| app.tick_demo(),
        );
        output.textures_delta.clear();
        assert_eq!(app.current_page, 0);
        assert_eq!(
            app.control.demo.as_ref().unwrap().progress.status,
            Status::Cancelled
        );
        let nested = script(vec![Step::Command {
            command: api::Command::RunDemo {
                script: script(vec![Step::Pause { ms: 100 }]),
            },
        }]);
        assert!(nested.validate().is_err());
        let mut player = Player::new(
            script(vec![Step::WaitView { timeout_ms: 100 }]),
            &app.control_state(),
        )
        .unwrap();
        player.next(0.0, &app.control_state());
        player.next(0.2, &app.control_state());
        assert_eq!(player.progress.status, Status::Failed);
    }
}
