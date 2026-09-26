use std::fmt::Write as _;
use std::sync::Arc;

use bevy::prelude::*;
use bevy_egui::EguiContexts;
use bevy_egui::egui::{
    self, Align, Align2, Color32, CursorIcon, FontId, Id, Layout, Rect, RichText, Sense, Stroke,
    StrokeKind, TextEdit, Vec2, pos2, vec2,
};
use world::Install;

use super::catalog::{Catalog, GROUND, KINDS};
use super::pictures::{Pictures, Shown};
use super::{Armed, Order, Palette, Pick, SIDES, Tab, WIDTHS};
use crate::catalog::what_fits;

pub const FILL: Color = Color::srgb_u8(24, 20, 16);
const PANEL: Color32 = Color32::from_rgb(24, 20, 16);
const CELL: Color32 = Color32::from_rgb(38, 33, 27);
const WELL: Color32 = Color32::from_rgb(12, 10, 8);
const GOLD: Color32 = Color32::from_rgb(255, 209, 0);
const TEXT: Color32 = Color32::from_rgb(232, 226, 212);
const DIM: Color32 = Color32::from_rgb(150, 142, 128);
const TROUBLE: Color32 = Color32::from_rgb(230, 110, 90);
const GAP: f32 = 6.0;
const NAME: f32 = 16.0;
const EDGE: f32 = 5.0;
/// The most of the frame the panel may take.
const MOST_OF_THE_FRAME: f32 = 0.7;
const FONTS: [(&str, &str); 2] = [
    ("frizqt", "Fonts\\FRIZQT__.TTF"),
    ("arialn", "Fonts\\ARIALN.TTF"),
];

/// The install's own interface fonts, each checked as egui will read it.
pub fn fonts(install: &Install) -> Result<egui::FontDefinitions, String> {
    let mut defs = egui::FontDefinitions::empty();
    for (name, path) in FONTS {
        let bytes = install
            .0
            .read(path)
            .map_err(|e| format!("the install's {path}: {e}"))?;
        font(&bytes).map_err(|e| format!("the install's {path}: {e}"))?;
        let data = egui::FontData::from_owned(bytes);
        defs.font_data.insert(name.to_owned(), Arc::new(data));
    }
    let names = |first: usize| vec![FONTS[first].0.to_owned(), FONTS[1 - first].0.to_owned()];
    defs.families
        .insert(egui::FontFamily::Proportional, names(0));
    defs.families.insert(egui::FontFamily::Monospace, names(1));
    Ok(defs)
}

/// Whether `bytes` read as the font egui takes them for.
pub fn font(bytes: &[u8]) -> Result<(), String> {
    ab_glyph::FontRef::try_from_slice_and_index(bytes, 0)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn style() -> egui::Style {
    let mut style = egui::Style {
        visuals: egui::Visuals::dark(),
        ..egui::Style::default()
    };
    let v = &mut style.visuals;
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.extreme_bg_color = WELL;
    v.override_text_color = Some(TEXT);
    v.selection.bg_fill = Color32::from_rgb(110, 84, 18);
    v.selection.stroke = Stroke::new(1.0_f32, GOLD);
    for (text, size) in [
        (egui::TextStyle::Heading, 18.0),
        (egui::TextStyle::Body, 14.0),
        (egui::TextStyle::Button, 14.0),
        (egui::TextStyle::Small, 11.0),
        (egui::TextStyle::Monospace, 13.0),
    ] {
        let family = if text == egui::TextStyle::Monospace {
            egui::FontFamily::Monospace
        } else {
            egui::FontFamily::Proportional
        };
        style.text_styles.insert(text, FontId::new(size, family));
    }
    style.spacing.item_spacing = vec2(GAP, GAP);
    style
}

/// The panel, once the fonts are in: its spot, tabs, search, order and the grid of pictures.
pub fn draw(
    mut contexts: EguiContexts<'_, '_>,
    mut palette: ResMut<'_, Palette>,
    mut pictures: ResMut<'_, Pictures>,
    mut armed: ResMut<'_, Armed>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let palette = &mut *palette;
    pictures.begin_pass();
    if !palette.fonts_set {
        if let Some(Ok(defs)) = &palette.fonts {
            ctx.set_fonts(defs.clone());
            ctx.set_style(style());
            ctx.options_mut(|o| o.zoom_with_keyboard = false);
            palette.fonts_set = true;
        }
        return Ok(());
    }
    if !palette.open {
        return Ok(());
    }
    let closing =
        ctx.input(|i| i.modifiers.ctrl && i.modifiers.shift && i.key_pressed(egui::Key::P));
    if closing && ctx.wants_keyboard_input() {
        palette.open = false;
        return Ok(());
    }
    pictures.at_side((palette.side * ctx.pixels_per_point()).round() as u32);
    let frame = egui::Frame::new().fill(PANEL).inner_margin(8.0);
    egui::CentralPanel::default().frame(frame).show(ctx, |ui| {
        resize(ui, palette);
        head(ui, palette, &armed);
        controls(ui, palette);
        grid(ui, palette, &mut pictures, &mut armed);
    });
    palette.drawn = Some(palette.key());
    Ok(())
}

/// The panel's left edge, dragged to widen or narrow it.
fn resize(ui: &mut egui::Ui, palette: &mut Palette) {
    let whole = ui.ctx().viewport_rect();
    let edge = Rect::from_min_max(whole.min, pos2(whole.min.x + EDGE, whole.max.y));
    let dragged = ui.interact(edge, Id::new("palette edge"), Sense::drag());
    if dragged.hovered() || dragged.dragged() {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
    }
    if dragged.dragged() {
        let frame = whole.max.x;
        let most = (frame * MOST_OF_THE_FRAME).max(*WIDTHS.start());
        palette.width = (palette.width - dragged.drag_delta().x).clamp(*WIDTHS.start(), most);
    }
}

fn head(ui: &mut egui::Ui, palette: &mut Palette, armed: &Armed) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Palette").color(GOLD).heading());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new("Ctrl+Shift+P").color(DIM).small());
        });
    });
    let (said, colour) = spot_line(palette);
    ui.label(RichText::new(said).color(colour).small());
    ui.horizontal(|ui| {
        ui.checkbox(&mut palette.follows, "follow the camera");
        if let Some(pick) = &armed.0 {
            let (Pick::Model(path) | Pick::Ground(path)) = pick;
            let name = what_fits::stem(path);
            ui.label(RichText::new(format!("armed: {name}")).color(GOLD));
        }
    });
}

fn spot_line(palette: &Palette) -> (String, Color32) {
    if let Some(e) = palette.failed() {
        return (e.to_owned(), TROUBLE);
    }
    let Some(catalog) = palette.catalog() else {
        let said = format!("reading the catalog in {}", palette.dir().display());
        return (said, DIM);
    };
    if let Some(trouble) = &palette.trouble {
        return (format!("the spot: {trouble}"), TROUBLE);
    }
    let Some(ranked) = &palette.ranked else {
        return ("looking for the spot the camera looks at".to_owned(), DIM);
    };
    let spot = &ranked.found.spot;
    let zone = spot
        .zone
        .map_or("no zone", |z| catalog.tables.zones[z].name.as_str());
    let mut said = format!("{:.1}, {:.1} in {zone}", ranked.at[0], ranked.at[1]);
    if let Some(under) = &ranked.found.under {
        let _ = write!(said, ", on {}", what_fits::stem(under));
    }
    if let Some((_, band)) = spot.ground {
        let _ = write!(said, " at {} degrees", fits::band_name(band));
    }
    let around = spot.near.len();
    let _ = write!(said, "; {around} things within {} yd", fits::AROUND);
    (said, DIM)
}

fn controls(ui: &mut egui::Ui, palette: &mut Palette) {
    ui.horizontal_wrapped(|ui| {
        for tab in palette.tabs() {
            let chosen = palette.tab == tab;
            let named = matches!(tab, Tab::List(_));
            let mut text = RichText::new(tab_name(&tab));
            if named {
                text = text.color(GOLD);
            }
            if ui.selectable_label(chosen, text).clicked() && !chosen {
                palette.choose(tab);
            }
        }
    });
    ui.horizontal(|ui| {
        let search = TextEdit::singleline(&mut palette.search)
            .hint_text("search names, paths and zones")
            .desired_width(ui.available_width() - 28.0);
        ui.add(search);
        if ui.button("x").clicked() {
            palette.search.clear();
        }
    });
    ui.horizontal(|ui| {
        for &order in palette.orders() {
            ui.selectable_value(&mut palette.order, order, order_name(order));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add(egui::Slider::new(&mut palette.side, SIDES).show_value(false));
            ui.label(RichText::new("size").color(DIM));
        });
    });
}

pub fn tab_name(tab: &Tab) -> String {
    match tab {
        Tab::Every => "all".to_owned(),
        Tab::Kind(GROUND) => "ground".to_owned(),
        Tab::Kind(k) => format!("{}s", KINDS[*k]),
        Tab::Recent => "recent".to_owned(),
        Tab::List(name) => name.clone(),
    }
}

pub fn order_name(order: Order) -> &'static str {
    match order {
        Order::Fits => "fits here",
        Order::Plain => "most placed",
        Order::Listed => "as listed",
    }
}

fn grid(ui: &mut egui::Ui, palette: &mut Palette, pictures: &mut Pictures, armed: &mut Armed) {
    let Some(catalog) = palette.catalog().cloned() else {
        return;
    };
    let shown = palette.shown();
    ui.label(
        RichText::new(format!("{} shown", shown.len()))
            .color(DIM)
            .small(),
    );
    let side = palette.side;
    let columns = ((ui.available_width() + GAP) / (side + GAP))
        .floor()
        .max(1.0) as usize;
    let rows = shown.len().div_ceil(columns);
    egui::ScrollArea::vertical().auto_shrink(false).show_rows(
        ui,
        side + NAME,
        rows,
        |ui, range| {
            for row in range {
                ui.horizontal(|ui| {
                    for &item in shown.iter().skip(row * columns).take(columns) {
                        cell(ui, &catalog, item, palette, pictures, armed);
                    }
                });
            }
        },
    );
}

fn cell(
    ui: &mut egui::Ui,
    catalog: &Catalog,
    item: usize,
    palette: &mut Palette,
    pictures: &mut Pictures,
    armed: &mut Armed,
) {
    let side = palette.side;
    let (rect, response) = ui.allocate_exact_size(vec2(side, side + NAME), Sense::click());
    let picture = Rect::from_min_size(rect.min, Vec2::splat(side));
    let painter = ui.painter_at(rect);
    painter.rect_filled(picture, 3.0, CELL);
    let it = &catalog.items[item];
    match pictures.show(item, || catalog.dir.join(&it.picture)) {
        Shown::Picture(id) => {
            egui::Image::new((id, picture.size())).paint_at(ui, picture);
        }
        Shown::Coming => {}
        Shown::Missing => {
            let font = FontId::proportional(11.0);
            painter.text(
                picture.center(),
                Align2::CENTER_CENTER,
                "no picture",
                font,
                DIM,
            );
        }
    }
    let name = ui.painter().layout(
        it.name().to_owned(),
        FontId::proportional(11.0),
        TEXT,
        f32::INFINITY,
    );
    let at = pos2(rect.min.x + 2.0, picture.max.y + 2.0);
    painter.with_clip_rect(rect).galley(at, name, TEXT);
    let picked = armed.0.as_ref().is_some_and(|p| {
        let (Pick::Model(path) | Pick::Ground(path)) = p;
        *path == it.path
    });
    if picked {
        painter.rect_stroke(picture, 3.0, Stroke::new(2.0_f32, GOLD), StrokeKind::Inside);
    } else if response.hovered() {
        painter.rect_stroke(picture, 3.0, Stroke::new(1.0_f32, TEXT), StrokeKind::Inside);
    }
    if response.clicked() {
        palette.pick(item, armed);
    }
    response.on_hover_ui(|ui| about(ui, catalog, item, palette));
}

/// What the tooltip says of a thing: its name, path and size, and why it stands where it does.
fn about(ui: &mut egui::Ui, catalog: &Catalog, item: usize, palette: &Palette) {
    let it = &catalog.items[item];
    ui.label(RichText::new(it.name()).color(GOLD));
    ui.label(RichText::new(&it.path).color(DIM).small());
    let placed = if it.kind == GROUND {
        format!("{} ground, painted in {} chunks", it.ground, it.placed)
    } else {
        format!(
            "{}, {}, placed {} times",
            KINDS[it.kind], it.size, it.placed
        )
    };
    ui.label(placed);
    if let Some(why) = why(catalog, item, palette) {
        ui.label(RichText::new(why).color(TEXT));
    }
    if let Some(said) = palette.said_of(item) {
        ui.label(RichText::new(said).color(GOLD));
    }
}

/// Why `item` stands where it does on the lists for the spot, when they are the order shown.
pub fn why(catalog: &Catalog, item: usize, palette: &Palette) -> Option<String> {
    let ranked = palette.ranked.as_ref()?;
    if palette.order != Order::Fits {
        return None;
    }
    if catalog.items[item].kind == GROUND {
        return ranked.ground_why.get(&item).cloned();
    }
    let model = catalog.model_of_item(item)?;
    let fit = ranked.fits.get(model)?;
    Some(what_fits::why(&catalog.tables, &ranked.found.spot, fit))
}
