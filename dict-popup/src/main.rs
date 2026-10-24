//! dict-popup — offline dictionary lookup with a floating card.
//!
//! Stage 0: prove the UI stack (egui/eframe on this machine) and measure the
//! memory footprint against the 100 MB budget. The real app replaces this
//! window with a hotkey-driven floating card.

struct App;

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("dict-popup");
            ui.add_space(8.0);
            ui.label("骨架已跑通：egui/eframe + serde + windows 依赖全部可用。");
            ui.label("下一步：热键 -> UIA 取词 -> StarDict 查询 -> 悬浮卡片。");
            ui.add_space(12.0);
            if ui.button("关闭").clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([420.0, 200.0])
            .with_title("dict-popup"),
        ..Default::default()
    };

    eframe::run_native(
        "dict-popup",
        options,
        Box::new(|_cc| Ok(Box::new(App) as Box<dyn eframe::App>)),
    )
}
