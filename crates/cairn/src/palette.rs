//! The palette: the catalog's pictures in a panel at the window's right, ranked by what fits the spot
//! the camera looks at, searched by words, with the things picked lately and the lists written into a
//! folder the window watches. A pick arms the thing for placing.

mod catalog;
mod lists;
mod panel;
mod pictures;
mod rank;
#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bevy::camera::visibility::VisibilitySystems;
use bevy::camera::{
    CameraOutputMode, CameraUpdateSystems, ClearColorConfig, RenderTarget, Viewport,
};
use bevy::core_pipeline::tonemapping::{DebandDither, Tonemapping};
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::prelude::*;
use bevy::render::view::Msaa;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, poll_once};
use bevy::transform::TransformSystems;
use bevy_egui::input::EguiWantsInput;
use bevy_egui::{
    EguiGlobalSettings, EguiPlugin, EguiPostUpdateSet, EguiPreUpdateSet, EguiPrimaryContextPass,
    PrimaryEguiContext,
};
use world::sight::Sight;
use world::{CurrentMap, FARCLIP, Install, Residency, WorldCamera};

use crate::args;
pub use catalog::{Catalog, GROUND, KINDS};
use lists::Lists;
pub use panel::why;
pub use pictures::Pictures;
pub use rank::Ranked;
use rank::Ranker;

/// Ctrl+Shift and this key open and close the palette.
pub const KEY: KeyCode = KeyCode::KeyP;
const SIDE: f32 = 96.0;
pub const SIDES: std::ops::RangeInclusive<f32> = 48.0..=192.0;
const WIDTH: f32 = 440.0;
pub const WIDTHS: std::ops::RangeInclusive<f32> = 240.0..=1200.0;
const RECENT: usize = 32;
const MET_NOTHING: &str = "the camera looks at no ground, so the lists are the last spot's";
/// How far the camera moves, or turns, before the spot is looked for again.
const MOVED_YD: f32 = 0.5;
const TURNED_COS: f32 = 0.999_96;

pub struct PalettePlugin {
    /// The catalog `cairn catalog` wrote.
    pub catalog: PathBuf,
    /// The folder of named lists.
    pub lists: Option<PathBuf>,
    pub map: args::Map,
}

/// What the palette shows and how.
#[derive(Resource)]
pub struct Palette {
    pub open: bool,
    pub tab: Tab,
    pub search: String,
    pub order: Order,
    /// The pictures' side, in points.
    pub side: f32,
    /// The panel's width, in points.
    pub width: f32,
    /// Whether the lists follow the spot the camera looks at.
    pub follows: bool,
    /// The items picked, the latest first.
    pub recent: Vec<usize>,
    dir: PathBuf,
    lists_dir: Option<PathBuf>,
    map: args::Map,
    loading: Loading,
    ranker: Slot,
    pub ranked: Option<Ranked>,
    /// From the ray to the lists being taken in, for the lists shown.
    pub took: Option<Duration>,
    looked: Option<Look>,
    pub lists: Option<Lists>,
    pub trouble: Option<String>,
    /// The place in the grid of the thing to bring to its top row.
    pub scroll_to: Option<usize>,
    /// How long the panel's last pass took to lay out.
    pub pass: Duration,
    fonts: Option<Result<bevy_egui::egui::FontDefinitions, String>>,
    fonts_set: bool,
    news: u64,
    shown: Option<(Key, Arc<Vec<usize>>)>,
    drawn: Option<Key>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tab {
    /// Every kind of model.
    Every,
    /// One of [`KINDS`].
    Kind(usize),
    Recent,
    /// A named list, by its name.
    List(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    /// What fits the spot the camera looks at first.
    Fits,
    /// The most placed first, as the catalog's pages go.
    Plain,
    /// A named list's own order.
    Listed,
}

/// Whether the palette shows all it was asked to, as of the last frame: its catalog read, its lists
/// made for the spot the camera looks at once the world around it arrived, and its grid drawn with
/// its pictures.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Settled(pub bool);

/// What placing puts down next: a model or a ground texture, by its install path.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct Armed(pub Option<Pick>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pick {
    Model(String),
    Ground(String),
}

enum Loading {
    Unasked,
    Reading(Task<Result<Box<Ranker>, String>>),
    Ready(Arc<Catalog>),
    Failed(String),
}

enum Slot {
    Empty,
    Idle(Box<Ranker>),
    Busy(Ranking),
}

/// The ranker at work on a spot, and what it makes of it.
type Ranking = Task<(Box<Ranker>, Result<Option<Ranked>, String>)>;

/// Where the camera stood and looked when the spot was last looked for, and whether the world
/// around it had arrived.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Look {
    eye: Vec3,
    forward: Vec3,
    arrived: bool,
}

/// What the grid shows, as far as its order goes.
#[derive(Clone, Debug, PartialEq)]
struct Key {
    tab: Tab,
    order: Order,
    search: String,
    news: u64,
}

/// The camera the panel is drawn through: over the world, inside the panel alone.
#[derive(Component)]
struct PanelView;

impl Plugin for PalettePlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<EguiPlugin>() {
            app.add_plugins(EguiPlugin::default());
        }
        app.insert_resource(EguiGlobalSettings {
            auto_create_primary_context: false,
            enable_absorb_bevy_input_system: true,
            ..EguiGlobalSettings::default()
        })
        .insert_resource(Palette::new(self))
        .init_resource::<Pictures>()
        .init_resource::<Armed>()
        .init_resource::<Settled>()
        .add_systems(Startup, (spawn_view, read_fonts))
        .add_systems(
            PreUpdate,
            hold_the_pointer.after(EguiPreUpdateSet::ProcessInput),
        )
        .add_systems(Update, (toggle, take_in).chain())
        .add_systems(
            PostUpdate,
            (
                look_at_the_spot
                    .after(TransformSystems::Propagate)
                    .after(VisibilitySystems::CheckVisibility),
                place_view.before(CameraUpdateSystems),
                (pictures::load, settle)
                    .chain()
                    .after(EguiPostUpdateSet::EndPass),
            ),
        )
        .add_systems(EguiPrimaryContextPass, panel::draw);
    }
}

impl Palette {
    fn new(plugin: &PalettePlugin) -> Self {
        Self {
            open: false,
            tab: Tab::Every,
            search: String::new(),
            order: Order::Fits,
            side: SIDE,
            width: WIDTH,
            follows: true,
            recent: Vec::new(),
            dir: plugin.catalog.clone(),
            lists_dir: plugin.lists.clone(),
            map: plugin.map.clone(),
            loading: Loading::Unasked,
            ranker: Slot::Empty,
            ranked: None,
            took: None,
            looked: None,
            lists: None,
            trouble: None,
            scroll_to: None,
            pass: Duration::ZERO,
            fonts: None,
            fonts_set: false,
            news: 0,
            shown: None,
            drawn: None,
        }
    }

    pub fn catalog(&self) -> Option<&Arc<Catalog>> {
        match &self.loading {
            Loading::Ready(catalog) => Some(catalog),
            _ => None,
        }
    }

    /// Why the palette can't show the catalog, if it can't.
    pub fn failed(&self) -> Option<&str> {
        match (&self.loading, &self.fonts) {
            (Loading::Failed(e), _) | (_, Some(Err(e))) => Some(e),
            _ => None,
        }
    }

    pub fn dir(&self) -> &std::path::Path {
        &self.dir
    }

    /// The items the grid shows, in its order.
    pub fn shown(&mut self) -> Arc<Vec<usize>> {
        let key = self.key();
        if let Some((k, items)) = &self.shown
            && *k == key
        {
            return Arc::clone(items);
        }
        let items = Arc::new(self.catalog().map_or_else(Vec::new, |c| self.order_of(c)));
        self.shown = Some((key, Arc::clone(&items)));
        items
    }

    fn key(&self) -> Key {
        Key {
            tab: self.tab.clone(),
            order: self.order,
            search: self.search.clone(),
            news: self.news,
        }
    }

    /// The orders the tab offers, its own first.
    pub fn orders(&self) -> &'static [Order] {
        match self.tab {
            Tab::Recent => &[],
            Tab::List(_) => &[Order::Listed, Order::Fits, Order::Plain],
            Tab::Every | Tab::Kind(_) => &[Order::Fits, Order::Plain],
        }
    }

    pub fn tabs(&self) -> Vec<Tab> {
        let mut tabs = vec![Tab::Every];
        tabs.extend((0..KINDS.len()).map(Tab::Kind));
        tabs.push(Tab::Recent);
        if let Some(lists) = &self.lists {
            tabs.extend(lists.all.keys().cloned().map(Tab::List));
        }
        tabs
    }

    /// Shows `tab` in its own order.
    pub fn choose(&mut self, tab: Tab) {
        self.order = match tab {
            Tab::List(_) => Order::Listed,
            _ if self.order == Order::Listed => Order::Fits,
            _ => self.order,
        };
        self.tab = tab;
    }

    /// Arms `item` and keeps it first among the recent.
    pub fn pick(&mut self, item: usize, armed: &mut Armed) {
        let Some(catalog) = self.catalog() else {
            return;
        };
        let it = &catalog.items[item];
        armed.0 = Some(if it.kind == GROUND {
            Pick::Ground(it.path.clone())
        } else {
            Pick::Model(it.path.clone())
        });
        self.recent.retain(|&i| i != item);
        self.recent.insert(0, item);
        self.recent.truncate(RECENT);
        self.news += 1;
    }

    fn order_of(&self, catalog: &Catalog) -> Vec<usize> {
        let words = catalog::words(&self.search);
        let ranked = self.ranked.as_ref().filter(|_| self.order == Order::Fits);
        let order: Vec<usize> = match &self.tab {
            Tab::Recent => self.recent.clone(),
            Tab::List(name) => self.listed(catalog, name, ranked),
            Tab::Every => match ranked {
                Some(r) => r.models[0].clone(),
                None => catalog.plain(None),
            },
            Tab::Kind(GROUND) => match ranked {
                Some(r) => r.ground.clone(),
                None => catalog.plain(Some(GROUND)),
            },
            Tab::Kind(k) => match ranked {
                Some(r) => r.models[k + 1].clone(),
                None => catalog.plain(Some(*k)),
            },
        };
        let mut found: Vec<(catalog::Match, usize)> = order
            .into_iter()
            .filter_map(|i| Some((catalog.items[i].found(&words)?, i)))
            .collect();
        found.sort_by_key(|&(how, _)| how);
        found.into_iter().map(|(_, i)| i).collect()
    }

    fn listed(&self, catalog: &Catalog, name: &str, ranked: Option<&Ranked>) -> Vec<usize> {
        let Some(list) = self.lists.as_ref().and_then(|l| l.all.get(name)) else {
            return Vec::new();
        };
        let mut items: Vec<usize> = Vec::new();
        for (path, _) in &list.lines {
            if let Some(i) = catalog.find(path)
                && !items.contains(&i)
            {
                items.push(i);
            }
        }
        match (self.order, ranked) {
            (Order::Plain, _) => items.sort_by(|&a, &b| catalog.plainly(a, b)),
            (Order::Fits, Some(r)) => {
                let place = |i: usize| {
                    r.models[0]
                        .iter()
                        .chain(&r.ground)
                        .position(|&x| x == i)
                        .unwrap_or(usize::MAX)
                };
                items.sort_by_key(|&i| place(i));
            }
            _ => {}
        }
        items
    }

    /// What a named list says of `item`.
    pub fn said_of(&self, item: usize) -> Option<&str> {
        let (Tab::List(name), Some(catalog)) = (&self.tab, self.catalog()) else {
            return None;
        };
        let list = self.lists.as_ref()?.all.get(name)?;
        list.lines
            .iter()
            .find(|(path, said)| !said.is_empty() && catalog.find(path) == Some(item))
            .map(|(_, said)| said.as_str())
    }

    /// The lines of the named list shown that name nothing in the catalog.
    pub fn unknown(&self) -> Vec<&str> {
        let (Tab::List(name), Some(catalog)) = (&self.tab, self.catalog()) else {
            return Vec::new();
        };
        let Some(list) = self.lists.as_ref().and_then(|l| l.all.get(name)) else {
            return Vec::new();
        };
        list.lines
            .iter()
            .filter(|(path, _)| catalog.find(path).is_none())
            .map(|(path, _)| path.as_str())
            .collect()
    }

    /// Whether the palette shows all it was asked to: its catalog read, its spot's lists made for
    /// where the camera stands, and its grid drawn with them.
    fn settled(&self, pictures: &Pictures, camera: Option<&GlobalTransform>) -> bool {
        if !self.open || self.failed().is_some() {
            return true;
        }
        let reading = matches!(self.loading, Loading::Unasked | Loading::Reading(_));
        let ranking = matches!(self.ranker, Slot::Busy(_));
        let looked_here = !self.follows
            || camera.is_some_and(|c| {
                self.looked
                    .is_some_and(|l| l.arrived && l.eye == c.translation())
            });
        !reading
            && !ranking
            && looked_here
            && self.scroll_to.is_none()
            && self.fonts_set
            && self.drawn.as_ref() == Some(&self.key())
            && pictures.settled()
    }

    fn start_loading(&mut self, install: &Install, map: &CurrentMap) {
        if !matches!(self.loading, Loading::Unasked) {
            return;
        }
        let (dir, install, map, opened) = (
            self.dir.clone(),
            install.clone(),
            map.clone(),
            self.map.clone(),
        );
        let task = AsyncComputeTaskPool::get()
            .spawn(async move { Ranker::open(&dir, install, &map, &opened).map(Box::new) });
        self.loading = Loading::Reading(task);
        self.lists = Some(Lists::watch(self.lists_dir.clone()));
    }
}

impl Look {
    fn same(&self, other: &Look) -> bool {
        self.arrived == other.arrived
            && self.eye.distance(other.eye) < MOVED_YD
            && self.forward.dot(other.forward) > TURNED_COS
    }
}

fn spawn_view(mut commands: Commands<'_, '_>) {
    commands.spawn((
        Camera2d,
        Camera {
            order: 1,
            is_active: false,
            output_mode: CameraOutputMode::Write {
                blend_state: None,
                clear_color: ClearColorConfig::None,
            },
            clear_color: ClearColorConfig::Custom(panel::FILL),
            ..Camera::default()
        },
        Msaa::Off,
        Tonemapping::None,
        DebandDither::Disabled,
        PrimaryEguiContext,
        PanelView,
    ));
}

fn read_fonts(install: Option<Res<'_, Install>>, mut palette: ResMut<'_, Palette>) {
    palette.fonts = Some(match install {
        Some(install) => panel::fonts(&install),
        None => Err("no install to read the fonts from".into()),
    });
}

fn toggle(
    keys: Res<'_, ButtonInput<KeyCode>>,
    mut palette: ResMut<'_, Palette>,
    install: Option<Res<'_, Install>>,
    map: Option<Res<'_, CurrentMap>>,
) {
    let chord = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight])
        && keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    if chord && keys.just_pressed(KEY) {
        palette.open = !palette.open;
    }
    if palette.open
        && let (Some(install), Some(map)) = (install, map)
    {
        palette.start_loading(&install, &map);
    }
}

/// Takes in the catalog once read, each spot's lists once made, and the named lists that changed.
fn take_in(mut palette: ResMut<'_, Palette>) {
    let palette = &mut *palette;
    if let Loading::Reading(task) = &mut palette.loading
        && let Some(done) = block_on(poll_once(task))
    {
        match done {
            Ok(ranker) => {
                palette.loading = Loading::Ready(Arc::clone(&ranker.catalog));
                palette.ranker = Slot::Idle(ranker);
                palette.news += 1;
            }
            Err(e) => {
                warn!("the palette: {e}");
                palette.loading = Loading::Failed(e);
            }
        }
    }
    if let Slot::Busy(task) = &mut palette.ranker
        && let Some((ranker, done)) = block_on(poll_once(task))
    {
        palette.ranker = Slot::Idle(ranker);
        match done {
            Ok(Some(ranked)) => {
                palette.took = Some(ranked.took);
                palette.ranked = Some(ranked);
                palette.trouble = None;
                palette.news += 1;
            }
            Ok(None) => palette.trouble = Some(MET_NOTHING.to_owned()),
            Err(e) => palette.trouble = Some(e),
        }
    }
    if palette.lists.as_mut().is_some_and(Lists::take_news) {
        palette.news += 1;
    }
}

/// Looks for the spot the camera looks at when it has moved, or the world has arrived around it.
fn look_at_the_spot(
    mut palette: ResMut<'_, Palette>,
    sight: Sight<'_, '_>,
    camera: Query<'_, '_, &GlobalTransform, With<WorldCamera>>,
    residency: Res<'_, Residency>,
) {
    let palette = &mut *palette;
    if !palette.open || !palette.follows || !matches!(palette.ranker, Slot::Idle(_)) {
        return;
    }
    let Ok(placed) = camera.single() else {
        return;
    };
    let look = Look {
        eye: placed.translation(),
        forward: placed.forward().as_vec3(),
        arrived: residency.settled(),
    };
    if palette.looked.is_some_and(|l| l.same(&look)) {
        return;
    }
    let Slot::Idle(mut ranker) = std::mem::replace(&mut palette.ranker, Slot::Empty) else {
        return;
    };
    let ray = sight.cast(look.eye, placed.forward(), FARCLIP);
    let asked = Instant::now();
    let task = AsyncComputeTaskPool::get().spawn(async move {
        let done = ranker.rank(ray, asked);
        (ranker, done)
    });
    palette.ranker = Slot::Busy(task);
    palette.looked = Some(look);
}

type WorldView<'w, 's> = Query<
    'w,
    's,
    (&'static Camera, &'static RenderTarget),
    (With<WorldCamera>, Without<PanelView>),
>;

fn settle(
    palette: Res<'_, Palette>,
    pictures: Res<'_, Pictures>,
    camera: Query<'_, '_, &GlobalTransform, With<WorldCamera>>,
    mut settled: ResMut<'_, Settled>,
) {
    let now = Settled(palette.settled(&pictures, camera.single().ok()));
    if *settled != now {
        *settled = now;
    }
}

/// Draws the panel through its own camera, over the world camera's frame and inside the panel's
/// part of it, from the frame after its part is known.
fn place_view(
    palette: Res<'_, Palette>,
    world: WorldView<'_, '_>,
    mut view: Query<'_, '_, (&mut Camera, &mut RenderTarget), With<PanelView>>,
) {
    let (Ok((seen, target)), Ok((mut camera, mut own))) = (world.single(), view.single_mut())
    else {
        return;
    };
    if own.normalize(None) != target.normalize(None) {
        *own = target.clone();
    }
    let part = panel_part(seen, palette.width).filter(|_| palette.open);
    let placed = same_part(camera.viewport.as_ref(), part.as_ref());
    let active = part.is_some() && placed && palette.fonts_set;
    if camera.is_active != active {
        camera.is_active = active;
    }
    if part.is_some() && !placed {
        camera.viewport = part;
    }
}

fn same_part(a: Option<&Viewport>, b: Option<&Viewport>) -> bool {
    let corners = |v: &Viewport| (v.physical_position, v.physical_size);
    a.map(corners) == b.map(corners)
}

/// The panel's part of the world camera's frame: `width` points at its right.
pub fn panel_part(world: &Camera, width: f32) -> Option<Viewport> {
    let size = world.physical_target_size()?;
    let scale = world.target_scaling_factor().unwrap_or(1.0);
    let width = ((width * scale).round() as u32).clamp(1, size.x);
    Some(Viewport {
        physical_position: UVec2::new(size.x - width, 0),
        physical_size: UVec2::new(width, size.y),
        ..Viewport::default()
    })
}

/// While the pointer is over the panel, the wheel scrolls it and neither zooms nor turns the camera.
fn hold_the_pointer(
    wants: Res<'_, EguiWantsInput>,
    mut scroll: ResMut<'_, AccumulatedMouseScroll>,
    mut motion: ResMut<'_, AccumulatedMouseMotion>,
) {
    if !wants.wants_any_pointer_input() {
        return;
    }
    if scroll.delta != Vec2::ZERO {
        scroll.delta = Vec2::ZERO;
    }
    if motion.delta != Vec2::ZERO {
        motion.delta = Vec2::ZERO;
    }
}
