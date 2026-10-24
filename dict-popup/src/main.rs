//! dict-popup — offline dictionary lookup with a floating card.
//!
//! Stage 0 smoke test for the UI stack. The real card/settings UI replaces
//! this file's contents; `docs/DESIGN.md` is the contract.

struct App {
    close_clicked: bool,
}

impl Default for App {
    fn default() -> Self {
        Self {
            close_clicked: false,
        }
    }
}

impl eframe::App for App {
    // egui 0.37: `ui` is the required hook; `logic(ctx, frame)` runs before it.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("dict-popup");
            ui.add_space(6.0);
            ui.label("骨架已跑通：egui/eframe 0.37 + dict-core + dict-input。");
            ui.label("下一步：热键 → UIA 取词 → StarDict 查询 → 悬浮卡片。");
            ui.add_space(10.0);
            self.close_clicked = ui.button("关闭").clicked();
        });

        if self.close_clicked {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([460.0, 240.0])
            .with_title("dict-popup"),
        ..Default::default()
    };

    eframe::run_native(
        "dict-popup",
        options,
        Box::new(|_cc| Ok(Box::new(App::default()) as Box<dyn eframe::App>)),
    )
}
