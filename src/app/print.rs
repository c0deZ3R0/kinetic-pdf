//! The print window owns an immutable document snapshot and its live preview.
use super::*;
use crate::printing::{
    self, Colour, Configured, Duplex, Options, Orientation, PageFilter, Placement, Printer,
    Scaling, Snapshot,
};
use std::sync::atomic::AtomicBool;
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pages {
    All,
    Current,
    Selected,
    Range,
}
pub(super) struct PrintDialog {
    generation: u64,
    snapshot: Arc<Snapshot>,
    sizes: Vec<[f32; 2]>,
    selected: Vec<usize>,
    current: usize,
    options: Options,
    choice: Pages,
    range: String,
    filter: PageFilter,
    sheet: usize,
    names: Vec<String>,
    printer_name: String,
    printer: Option<Printer>,
    discovery: Option<Receiver<Result<(Vec<String>, String), String>>>,
    loading: Option<Receiver<Result<Printer, String>>>,
    setup: Option<Receiver<Result<Configured, String>>>,
    configured: Option<Configured>,
    texture: Option<TextureHandle>,
    positions: Vec<Placement>,
    key: Option<String>,
    serial: u64,
    due: f64,
    started: bool,
    error: Option<String>,
}
fn background<T: Send + 'static>(
    ctx: &egui::Context,
    work: impl FnOnce() -> T + Send + 'static,
) -> Receiver<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let _ = tx.send(work());
        ctx.request_repaint();
    });
    rx
}
impl PrintDialog {
    fn pages(&self) -> Result<Vec<usize>, String> {
        let pages = match self.choice {
            Pages::All => (0..self.sizes.len()).collect(),
            Pages::Current => vec![self.current],
            Pages::Selected => self.selected.clone(),
            Pages::Range => printing::page_range(&self.range, self.sizes.len())?,
        };
        let pages = printing::filter_pages(pages, self.filter, self.options.reverse);
        if pages.iter().any(|page| *page >= self.sizes.len()) {
            return Err("The document has no pages available for this selection".into());
        }
        if pages.is_empty() {
            return Err("No pages match this selection".into());
        }
        Ok(pages)
    }
    fn load(&mut self, ctx: &egui::Context, name: String) {
        self.printer_name = name.clone();
        self.printer = None;
        self.configured = None;
        self.texture = None;
        self.key = None;
        self.error = None;
        self.loading = Some(background(ctx, move || printing::load_printer(&name)));
    }
    fn adopt(&mut self, printer: Printer, properties: bool) {
        self.options.paper = printer.default_paper;
        if !printer.papers.iter().any(|p| p.id == self.options.paper) {
            if let Some(p) = printer.papers.first() {
                self.options.paper = p.id;
            }
        }
        if properties {
            self.options.orientation = if printer.landscape {
                Orientation::Landscape
            } else {
                Orientation::Portrait
            };
        }
        if self.options.colour != Colour::BlackWhite || printer.default_colour != Colour::Grayscale
        {
            self.options.colour = printer.default_colour;
        }
        self.options.duplex = printer.default_duplex;
        if !printer.colour {
            self.options.colour = Colour::Grayscale;
        }
        if !printer.duplex {
            self.options.duplex = Duplex::Single;
        }
        self.printer = Some(printer);
        self.key = None;
        self.error = None;
    }
    pub(super) fn receive_preview(
        &mut self,
        id: u64,
        serial: u64,
        result: Result<(TextureHandle, Vec<Placement>), String>,
    ) {
        if id != self.snapshot.id || serial != self.serial {
            return;
        }
        match result {
            Ok((texture, positions)) => {
                self.texture = Some(texture);
                self.positions = positions;
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }
    fn poll(&mut self, ctx: &egui::Context, tx: &Sender<Request>) {
        if let Some(result) = self.discovery.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.discovery = None;
            match result {
                Ok((names, default)) => {
                    self.names = names;
                    let selected = if self.names.contains(&default) {
                        default
                    } else {
                        self.names.first().cloned().unwrap_or_default()
                    };
                    if !selected.is_empty() {
                        self.load(ctx, selected);
                    } else {
                        self.error = Some(
                            "No printers are installed. Install a printer in Windows Settings."
                                .into(),
                        );
                    }
                }
                Err(error) => self.error = Some(error),
            }
        }
        if let Some(result) = self.loading.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.loading = None;
            match result {
                Ok(printer) => self.adopt(printer, false),
                Err(error) => self.error = Some(error),
            }
        }
        if let Some(result) = self.setup.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.setup = None;
            match result {
                Ok(configured) => {
                    if let Ok(pages) = self.pages() {
                        let pages: Vec<_> = pages
                            .chunks(self.options.pages_per_sheet)
                            .nth(self.sheet)
                            .unwrap_or(&[])
                            .to_vec();
                        if !pages.is_empty() {
                            let _ = tx.send(Request::PrintPreview {
                                generation: self.generation,
                                snapshot: self.snapshot.clone(),
                                configured: configured.clone(),
                                pages,
                                serial: self.serial,
                            });
                        }
                    }
                    self.configured = Some(configured);
                }
                Err(error) => self.error = Some(error),
            }
        }
    }
    fn queue_preview(&mut self, ctx: &egui::Context) {
        let Ok(pages) = self.pages() else {
            self.key = None;
            self.setup = None;
            self.configured = None;
            self.started = false;
            self.texture = None;
            return;
        };
        let groups = pages.len().div_ceil(self.options.pages_per_sheet);
        self.sheet = self.sheet.min(groups.saturating_sub(1));
        let pages = pages
            .chunks(self.options.pages_per_sheet)
            .nth(self.sheet)
            .unwrap_or(&[]);
        let Some(printer) = &self.printer else {
            return;
        };
        let key = format!("{}|{:?}|{:?}", printer.name, self.options, pages);
        let now = App::now(ctx);
        if self.key.as_ref() != Some(&key) {
            self.key = Some(key);
            self.serial += 1;
            self.due = now + 0.18;
            self.started = false;
            self.setup = None;
            self.configured = None;
            self.texture = None;
            self.positions.clear();
            self.error = None;
        }
        if now < self.due {
            ctx.request_repaint_after(Duration::from_secs_f64(self.due - now));
            return;
        }
        if !self.started {
            let printer = printer.clone();
            let mut options = self.options.clone();
            let source = self.sizes[pages[0]];
            if options.duplex != Duplex::Single
                && options.orientation == Orientation::Auto
                && self.sheet % 2 == 1
            {
                let selected = self.pages().unwrap();
                let front = selected[(self.sheet - 1) * options.pages_per_sheet];
                if let Ok((_, landscape)) =
                    printing::paper_for(&printer, &options, self.sizes[front])
                {
                    options.orientation = if landscape {
                        Orientation::Landscape
                    } else {
                        Orientation::Portrait
                    };
                }
            }
            self.setup = Some(background(ctx, move || {
                printing::configure(&printer, &options, source)
            }));
            self.started = true;
        }
    }
}
impl App {
    pub(super) fn show_print_options(&mut self) {
        if !self.action_enabled(palette::Action::Print) {
            return;
        }
        let Some(doc) = &self.doc else {
            return;
        };
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let snapshot = Arc::new(Snapshot {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            changes: doc.session.changes(self.author_name()),
            arrangement: doc.arrange.sheets().to_vec(),
            new_pages: doc.arrange.new_pages().to_vec(),
        });
        let sizes = super::arrange::sheet_sizes(doc)
            .iter()
            .map(|v| [v.x, v.y])
            .collect();
        self.print_options = Some(PrintDialog {
            generation: doc.generation,
            snapshot,
            sizes,
            selected: doc.arrange.selected().iter().copied().collect(),
            current: self
                .current_page
                .min(doc.arrange.sheets().len().saturating_sub(1)),
            options: Options::default(),
            choice: Pages::All,
            range: String::new(),
            filter: PageFilter::All,
            sheet: 0,
            names: Vec::new(),
            printer_name: String::new(),
            printer: None,
            discovery: Some(background(&self.ctx, printing::printers)),
            loading: None,
            setup: None,
            configured: None,
            texture: None,
            positions: Vec::new(),
            key: None,
            serial: 0,
            due: 0.,
            started: false,
            error: None,
        });
    }
    pub(super) fn print_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.print_options.take() else {
            return;
        };
        dialog.poll(ctx, &self.tx);
        let mut print = false;
        let mut cancel = false;
        let available = ctx.input(|i| i.content_rect().size());
        let width = (available.x - 64.).min(1040.).max(600.);
        let height = (available.y - 180.).min(700.).max(300.);
        let modal=egui::Modal::new(Id::new("print-pdf")).show(ctx,|ui|{
            ui.set_width(width);
            ui.horizontal(|ui|{ui.heading("Print PDF");ui.with_layout(Layout::right_to_left(Align::Center),|ui|{ui.label(format!("{} document pages",dialog.sizes.len()));});});
            ui.add_space(8.);
            // Modal Areas otherwise offer unbounded height: a vertical
            // separator in a horizontal row then expands the entire modal.
            ui.allocate_ui_with_layout(vec2(width,height),Layout::left_to_right(Align::TOP),|ui|{
                ui.vertical(|ui|{
                    ui.set_width(340.);
                    egui::ScrollArea::vertical().id_salt("print-controls").max_height(height).show(ui,|ui|{
                        section(ui,"Printer");
                        if dialog.discovery.is_some(){ui.horizontal(|ui|{ui.spinner();ui.label("Finding printers…");});}
                        let old=dialog.printer_name.clone();
                        egui::ComboBox::from_id_salt("printer-name").width(320.).selected_text(if dialog.printer_name.is_empty(){"Choose a printer"}else{&dialog.printer_name}).show_ui(ui,|ui|{
                            for name in &dialog.names{ui.selectable_value(&mut dialog.printer_name,name.clone(),name);}
                        });
                        if dialog.printer_name!=old{dialog.load(ctx,dialog.printer_name.clone());}
                        if dialog.loading.is_some(){ui.horizontal(|ui|{ui.spinner();ui.label("Reading printer capabilities…");});}
                        ui.horizontal(|ui|{
                            if ui.add_enabled(dialog.printer.is_some(),egui::Button::new("Printer Properties…")).clicked(){
                                let printer=dialog.configured.as_ref().map(|c|&c.printer).or(dialog.printer.as_ref()).unwrap();
                                match printing::properties(printer){Ok(Some(printer))=>dialog.adopt(printer,true),Ok(None)=>{},Err(error)=>dialog.error=Some(error)}
                            }
                            if ui.button("Refresh").clicked(){dialog.discovery=Some(background(ctx,printing::printers));}
                        });
                        ui.horizontal(|ui|{
                            ui.label("Copies");ui.add(egui::DragValue::new(&mut dialog.options.copies).range(1..=999));
                            ui.add_enabled(dialog.options.duplex==Duplex::Single,egui::Checkbox::new(&mut dialog.options.collate,"Collate"));
                        });
                        section(ui,"Pages");
                        ui.horizontal(|ui|{ui.radio_value(&mut dialog.choice,Pages::All,"All");ui.radio_value(&mut dialog.choice,Pages::Current,"Current");
                            ui.add_enabled(!dialog.selected.is_empty(),egui::RadioButton::new(dialog.choice==Pages::Selected,"Selected")).clicked().then(||dialog.choice=Pages::Selected);
                        });
                        ui.horizontal(|ui|{ui.radio_value(&mut dialog.choice,Pages::Range,"Range");ui.add_enabled(dialog.choice==Pages::Range,TextEdit::singleline(&mut dialog.range).hint_text("1-3, 7, 10-12").desired_width(220.));});
                        ui.horizontal(|ui|{
                            egui::ComboBox::from_id_salt("print-filter").selected_text(match dialog.filter{PageFilter::All=>"All selected pages",PageFilter::Odd=>"Odd pages only",PageFilter::Even=>"Even pages only"}).show_ui(ui,|ui|{
                                ui.selectable_value(&mut dialog.filter,PageFilter::All,"All selected pages");ui.selectable_value(&mut dialog.filter,PageFilter::Odd,"Odd pages only");ui.selectable_value(&mut dialog.filter,PageFilter::Even,"Even pages only");
                            });
                            ui.checkbox(&mut dialog.options.reverse,"Reverse order");
                        });
                        section(ui,"Paper & layout");
                        if let Some(printer)=&dialog.printer{
                            let selected=printer.papers.iter().find(|p|p.id==dialog.options.paper).map_or("Choose paper",|p|p.name.as_str());
                            ui.add_enabled_ui(!dialog.options.match_paper,|ui|{
                                egui::ComboBox::from_id_salt("print-paper").width(300.).selected_text(selected).show_ui(ui,|ui|{
                                    for paper in &printer.papers{ui.selectable_value(&mut dialog.options.paper,paper.id,format!("{} · {}",paper.name,self.units.paper_size(paper.mm.map(|mm| mm*72./25.4))));}
                                });
                            });
                        }
                        ui.add_enabled(dialog.options.pages_per_sheet==1,egui::Checkbox::new(&mut dialog.options.match_paper,"Match each document page's size"));
                        ui.horizontal(|ui|{
                            ui.label("Orientation");egui::ComboBox::from_id_salt("print-orientation").selected_text(match dialog.options.orientation{Orientation::Auto=>"Automatic",Orientation::Portrait=>"Portrait",Orientation::Landscape=>"Landscape"}).show_ui(ui,|ui|{
                                ui.selectable_value(&mut dialog.options.orientation,Orientation::Auto,"Automatic");ui.selectable_value(&mut dialog.options.orientation,Orientation::Portrait,"Portrait");ui.selectable_value(&mut dialog.options.orientation,Orientation::Landscape,"Landscape");
                            });
                        });
                        ui.horizontal(|ui|{
                            ui.label("Pages per sheet");egui::ComboBox::from_id_salt("print-nup").selected_text(dialog.options.pages_per_sheet.to_string()).show_ui(ui,|ui|{for n in [1,2,4,6,9,16]{ui.selectable_value(&mut dialog.options.pages_per_sheet,n,n.to_string());}});
                        });
                        if dialog.options.pages_per_sheet>1{dialog.options.match_paper=false;}
                        ui.add_enabled_ui(dialog.printer.as_ref().is_some_and(|p|p.duplex),|ui|{
                            ui.horizontal(|ui|{ui.label("Sides");egui::ComboBox::from_id_salt("print-duplex").selected_text(match dialog.options.duplex{Duplex::Single=>"Single-sided",Duplex::LongEdge=>"Double-sided · long edge",Duplex::ShortEdge=>"Double-sided · short edge"}).show_ui(ui,|ui|{
                                ui.selectable_value(&mut dialog.options.duplex,Duplex::Single,"Single-sided");ui.selectable_value(&mut dialog.options.duplex,Duplex::LongEdge,"Double-sided · flip on long edge");ui.selectable_value(&mut dialog.options.duplex,Duplex::ShortEdge,"Double-sided · flip on short edge");
                            });});
                        });
                        if dialog.options.duplex!=Duplex::Single{dialog.options.collate=true;}
                        section(ui,"Sizing");
                        ui.add_enabled_ui(dialog.options.pages_per_sheet==1,|ui|{
                            ui.horizontal(|ui|{ui.radio_value(&mut dialog.options.scaling,Scaling::Fit,"Fit");ui.radio_value(&mut dialog.options.scaling,Scaling::Shrink,"Shrink oversized");ui.radio_value(&mut dialog.options.scaling,Scaling::Actual,"Actual size");});
                            ui.horizontal(|ui|{ui.radio_value(&mut dialog.options.scaling,Scaling::Custom,"Custom scale");ui.add_enabled(dialog.options.scaling==Scaling::Custom,egui::DragValue::new(&mut dialog.options.percent).range(1.0..=1000.0).suffix("%"));});
                        });
                        ui.horizontal(|ui|{ui.checkbox(&mut dialog.options.auto_rotate,"Auto rotate");ui.checkbox(&mut dialog.options.center,"Centre on paper");});
                        section(ui,"Content & colour");
                        ui.horizontal(|ui|{
                            if ui.add_enabled(dialog.printer.as_ref().is_some_and(|p|p.colour),egui::RadioButton::new(dialog.options.colour==Colour::Colour,"Colour")).clicked(){dialog.options.colour=Colour::Colour;}
                            ui.radio_value(&mut dialog.options.colour,Colour::Grayscale,"Grayscale");ui.radio_value(&mut dialog.options.colour,Colour::BlackWhite,"Black & white");
                        });
                        if dialog.options.colour==Colour::BlackWhite{ui.small("Pure black and white removes grey shading; check the preview for lost detail.");}
                        ui.checkbox(&mut dialog.options.include_markups,"Include markups & measurements").on_hover_text("Includes saved and unsaved annotations, highlights and measurements. Flattened markups are part of the drawing and cannot be hidden.");
                        section(ui,"Detailed drawings");
                        ui.checkbox(&mut dialog.options.as_image,"Print as image").on_hover_text("Use this if a complex PDF prints slowly or incorrectly. The app renders the drawing in small image bands instead of sending its vectors to the printer.");
                        ui.add_enabled_ui(dialog.options.as_image||dialog.options.colour==Colour::BlackWhite,|ui|{
                            ui.horizontal(|ui|{ui.label("Image quality");egui::ComboBox::from_id_salt("print-dpi").selected_text(format!("{} DPI",dialog.options.dpi)).show_ui(ui,|ui|{
                                for (dpi,label) in [(150,"150 DPI · draft / smaller job"),(300,"300 DPI · balanced"),(600,"600 DPI · fine detail")]{ui.selectable_value(&mut dialog.options.dpi,dpi,label);}
                            });});
                        });
                        ui.small("Vector printing keeps fine lines sharp. Image printing uses bounded memory; higher DPI takes longer and creates larger jobs.");
                    });
                });
                ui.separator();
                ui.vertical(|ui|{
                    ui.set_width((width-370.).max(220.));
                    let pages=dialog.pages();
                    match &pages{
                        Ok(pages)=>{
                            let groups=pages.len().div_ceil(dialog.options.pages_per_sheet);dialog.sheet=dialog.sheet.min(groups.saturating_sub(1));
                            ui.horizontal(|ui|{
                                if ui.add_enabled(dialog.sheet>0,egui::Button::new("‹")).clicked(){dialog.sheet-=1;}
                                ui.label(format!("Preview · sheet {} of {}",dialog.sheet+1,groups));
                                if ui.add_enabled(dialog.sheet+1<groups,egui::Button::new("›")).clicked(){dialog.sheet+=1;}
                            });
                            let group=pages.chunks(dialog.options.pages_per_sheet).nth(dialog.sheet).unwrap_or(&[]);
                            ui.small(format!("Document pages {}",group.iter().map(|p|(p+1).to_string()).collect::<Vec<_>>().join(", ")));
                            if let Some(&page)=group.first(){let size=dialog.sizes[page];ui.small(format!("Source page: {}",self.units.paper_size(size)));}
                            let sheets=if dialog.options.duplex==Duplex::Single{groups}else{groups.div_ceil(2)}*usize::from(dialog.options.copies);
                            ui.small(format!("{} pages · {} copies · {} paper sheets",pages.len(),dialog.options.copies,sheets));
                        },Err(error)=>{ui.colored_label(DIRTY,error);}
                    }
                    ui.add_space(8.);
                    let preview_height=(height-130.).max(160.);
                    Frame::NONE.fill(BG).inner_margin(Margin::same(16)).show(ui,|ui|{
                        ui.set_min_size(vec2((width-406.).max(160.),preview_height));
                        if let (Some(texture),Some(configured))=(&dialog.texture,&dialog.configured){
                            let paper=configured.metrics.paper;let scale=((ui.available_width()-8.)/paper[0]).min((preview_height-16.)/paper[1]);
                            let size=vec2(paper[0]*scale,paper[1]*scale);
                            ui.vertical_centered(|ui|{
                                let response=ui.add(egui::Image::new(texture).fit_to_exact_size(size));
                                let [x,y,w,h]=configured.metrics.printable;
                                let area=Rect::from_min_size(response.rect.min+vec2(x,y)*scale,vec2(w,h)*scale);
                                ui.painter().rect_stroke(area,CornerRadius::ZERO,Stroke::new(1.,Color32::from_rgb(180,185,192)),StrokeKind::Inside);
                            });
                        }else{ui.vertical_centered(|ui|{ui.add_space(preview_height/3.);if dialog.error.is_none(){ui.spinner();ui.label("Preparing preview…");}});}
                    });
                    if let Some(config)=&dialog.configured{ui.small(format!("Paper: {} · grey outline = printable area",self.units.paper_size(config.metrics.paper)));}
                    if let Some(pos)=dialog.positions.first(){ui.small(format!("Scale: {:.1}%{}",pos.scale*100.,if pos.rotate{" · rotated to fit"}else{""}));}
                    if dialog.positions.iter().any(|p|p.rect[0]<p.clip[0]-0.1||p.rect[1]<p.clip[1]-0.1||p.rect[0]+p.rect[2]>p.clip[0]+p.clip[2]+0.1||p.rect[1]+p.rect[3]>p.clip[1]+p.clip[3]+0.1){ui.colored_label(DIRTY,"Content outside the printable area will be clipped.");}
                    if let Some(error)=&dialog.error{ui.colored_label(DIRTY,error);}
                });
            });
            ui.separator();
            ui.horizontal(|ui|{
                let fresh = dialog.pages().ok().and_then(|pages| {
                    let group = pages.chunks(dialog.options.pages_per_sheet).nth(dialog.sheet)?;
                    let printer = dialog.printer.as_ref()?;
                    Some(format!("{}|{:?}|{:?}",printer.name,dialog.options,group))
                }).is_some_and(|key|dialog.key.as_ref()==Some(&key));
                let ready=fresh&&dialog.configured.is_some()&&dialog.texture.is_some()&&dialog.error.is_none();
                print=ui.add_enabled(ready,egui::Button::new("Print").fill(ACCENT)).clicked();
                cancel=ui.button("Cancel").clicked();
                ui.small("Includes current unsaved edits. The PDF file is unchanged by printing.");
            });
        });
        dialog.queue_preview(ctx);
        if print {
            if let (Some(printer), Ok(pages)) = (dialog.printer.clone(), dialog.pages()) {
                let mut output = None;
                let pdf = printer.name.to_lowercase().contains("print to pdf");
                let xps = printer.name.to_lowercase().contains("xps document writer");
                if pdf || xps {
                    let extension = if pdf { "pdf" } else { "oxps" };
                    output = rfd::FileDialog::new()
                        .set_title("Save printed document")
                        .add_filter(if pdf { "PDF" } else { "XPS" }, &[extension])
                        .set_file_name(format!("Print.{extension}"))
                        .save_file();
                    if output.is_none() {
                        self.print_options = Some(dialog);
                        return;
                    }
                    if let (Some(output), Some(doc)) = (&output, &self.doc) {
                        if output
                            .canonicalize()
                            .ok()
                            .zip(doc.path.canonicalize().ok())
                            .is_some_and(|(a, b)| a == b)
                        {
                            dialog.error = Some(
                                "Choose a different output file to keep the open PDF intact."
                                    .into(),
                            );
                            self.print_options = Some(dialog);
                            return;
                        }
                    }
                }
                let cancelled = Arc::new(AtomicBool::new(false));
                let job = printing::Job {
                    printer,
                    options: dialog.options.clone(),
                    pages,
                    cancelled: cancelled.clone(),
                    output,
                };
                if self
                    .tx
                    .send(Request::Print {
                        generation: dialog.generation,
                        snapshot: dialog.snapshot.clone(),
                        job,
                    })
                    .is_ok()
                {
                    self.printing = true;
                    self.print_cancel = Some(cancelled);
                    self.print_progress = Some((0, 0));
                }
            }
        } else if !cancel && !modal.should_close() {
            self.print_options = Some(dialog);
        } else {
            let _ = self.tx.send(Request::EndPrintPreview {
                id: dialog.snapshot.id,
            });
        }
    }
    pub(super) fn print_progress_dialog(&mut self, ctx: &egui::Context) {
        if !self.printing {
            return;
        }
        let stopping = self
            .print_cancel
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Relaxed));
        egui::Modal::new(Id::new("print-progress")).show(ctx, |ui| {
            ui.set_width(360.);
            ui.heading(if stopping {
                "Cancelling print job…"
            } else {
                "Printing"
            });
            let (done, total) = self.print_progress.unwrap_or((0, 0));
            if total > 0 {
                ui.add(
                    egui::ProgressBar::new(done as f32 / total as f32)
                        .text(format!("{done} of {total} printed sides")),
                );
            } else {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Preparing document and printer…");
                });
            }
            if ui
                .add_enabled(!stopping, egui::Button::new("Cancel printing"))
                .clicked()
            {
                if let Some(flag) = &self.print_cancel {
                    flag.store(true, Ordering::Relaxed);
                }
            }
            ui.small(
                "Cancellation stops the remaining job. Pages already printed cannot be recalled.",
            );
        });
    }
}
fn section(ui: &mut Ui, label: &str) {
    ui.add_space(10.);
    ui.label(RichText::new(label).strong());
    ui.add_space(3.);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn print_dialog_stays_inside_the_window() {
        let (mut app, ctx) = super::super::tests::app_with_a_document();
        app.show_print_options();
        // Keep this layout check independent of installed printer drivers.
        app.print_options.as_mut().unwrap().discovery = None;
        for size in [vec2(1280., 900.), vec2(800., 600.), vec2(1024., 768.)] {
            let screen = Rect::from_min_size(egui::Pos2::ZERO, size);
            for _ in 0..4 {
                let input = egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                };
                let mut output = ctx.run_ui(input, |ui| app.print_dialog(ui.ctx()));
                output.textures_delta.clear();
            }
            let rect = ctx
                .memory(|memory| memory.area_rect(Id::new("print-pdf")))
                .unwrap();
            assert!(
                screen.contains_rect(rect),
                "Print dialog {rect:?} exceeds window {screen:?}"
            );
        }
    }
}
