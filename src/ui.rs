use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use gpui::prelude::FluentBuilder;
use gpui::{
    Animation, AnimationExt, AnyElement, App, AppContext, Bounds, ClipboardItem, Context, Corner,
    Entity, FocusHandle, Focusable, InteractiveElement, IntoElement, KeyBinding, KeyDownEvent,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, PathBuilder, Pixels, Point,
    Render, ScrollDelta, ScrollHandle, ScrollWheelEvent, SharedString, StatefulInteractiveElement,
    Styled, Subscription, WeakEntity, Window, WindowBounds, WindowControlArea, WindowOptions,
    actions, canvas, div, ease_out_quint, img, point, px, size,
};
use gpui_component::{
    Disableable, Icon, IconName, Root, Sizable, Theme, ThemeMode, TitleBar, WindowExt,
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
    list::ListItem,
    menu::{DropdownMenu, PopupMenuItem},
    notification::Notification,
    resizable::{ResizableState, h_resizable, resizable_panel},
    scroll::ScrollableElement,
    skeleton::Skeleton,
    tab::TabBar,
    table::{Column, ColumnSort, Table, TableDelegate, TableEvent, TableState},
    tree::{TreeItem, TreeState, tree},
};
use serde::{Deserialize, Serialize};
use tokio::runtime::Runtime;

use crate::{
    audit::{
        AuditFinding, Section, SnapshotChange, Tone, WorkerAuditPreference, WorkerAuditPreferences,
        build_audit_findings, build_audit_report, compact_number, diff_snapshots, format_bytes,
        money,
    },
    backend::{Account, Backend, ConnectionState, DashboardSnapshot, ResourceRow},
    design::{FONT_MONO, Palette, apply_component_theme, color},
    updater::{self, UpdateStatus},
};
const THEME_KEY: &str = "gpui_theme";
const SIDEBAR_COLLAPSED_KEY: &str = "sidebar_collapsed";
const WORKER_PREFERENCES_KEY: &str = "worker_audit_preferences";
const INSPECTOR_WIDTH_KEY: &str = "inspector_width";
const REDUCED_MOTION_KEY: &str = "reduced_motion";
const WORKSPACE_STATE_KEY: &str = "workspace_state_v1";
const WINDOW_STATE_KEY: &str = "window_state_v1";
const AUTOMATIC_UPDATE_CHECKS_KEY: &str = "automatic_update_checks";
const INSPECTOR_HISTORY_LIMIT: usize = 50;
const TOPOLOGY_NODE_WIDTH: f32 = 176.;
const TOPOLOGY_NODE_HEIGHT: f32 = 62.;

pub(crate) const VISUAL_QA_SCENARIOS: &[&str] = &[
    "audit",
    "resources-table",
    "resources-topology",
    "workers-inspector",
    "cost",
    "connection",
    "settings",
    "command-palette",
    "shortcuts",
    "empty-resources",
    "loading",
    "error",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum VisualQaScenario {
    Audit,
    ResourcesTable,
    ResourcesTopology,
    WorkersInspector,
    Cost,
    Connection,
    Settings,
    CommandPalette,
    Shortcuts,
    EmptyResources,
    Loading,
    Error,
}

impl VisualQaScenario {
    fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "audit" => Ok(Self::Audit),
            "resources-table" => Ok(Self::ResourcesTable),
            "resources-topology" => Ok(Self::ResourcesTopology),
            "workers-inspector" => Ok(Self::WorkersInspector),
            "cost" => Ok(Self::Cost),
            "connection" => Ok(Self::Connection),
            "settings" => Ok(Self::Settings),
            "command-palette" => Ok(Self::CommandPalette),
            "shortcuts" => Ok(Self::Shortcuts),
            "empty-resources" => Ok(Self::EmptyResources),
            "loading" => Ok(Self::Loading),
            "error" => Ok(Self::Error),
            _ => anyhow::bail!(
                "unknown visual QA scenario '{value}'; expected one of: {}",
                VISUAL_QA_SCENARIOS.join(", ")
            ),
        }
    }

    fn name(self) -> &'static str {
        VISUAL_QA_SCENARIOS[self as usize]
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct VisualQaConfig {
    scenario: VisualQaScenario,
    dark: bool,
    width: f32,
    height: f32,
}

impl VisualQaConfig {
    pub(crate) fn from_args(args: &[String]) -> anyhow::Result<Option<Self>> {
        let Some(index) = args.iter().position(|argument| argument == "--visual-qa") else {
            return Ok(None);
        };
        let scenario = args
            .get(index + 1)
            .ok_or_else(|| anyhow::anyhow!("--visual-qa requires a scenario"))
            .and_then(|value| VisualQaScenario::parse(value))?;
        let mut config = Self {
            scenario,
            dark: true,
            width: 1440.,
            height: 960.,
        };
        if let Some(theme_index) = args.iter().position(|argument| argument == "--theme") {
            config.dark = match args.get(theme_index + 1).map(String::as_str) {
                Some("dark") => true,
                Some("light") => false,
                Some(value) => anyhow::bail!("unknown visual QA theme '{value}'"),
                None => anyhow::bail!("--theme requires dark or light"),
            };
        }
        if let Some(viewport_index) = args.iter().position(|argument| argument == "--viewport") {
            let viewport = args
                .get(viewport_index + 1)
                .ok_or_else(|| anyhow::anyhow!("--viewport requires WIDTHxHEIGHT"))?;
            let (width, height) = viewport
                .split_once(['x', 'X'])
                .ok_or_else(|| anyhow::anyhow!("invalid viewport '{viewport}'"))?;
            config.width = width.parse::<f32>()?;
            config.height = height.parse::<f32>()?;
            if config.width < 1120.
                || config.height < 720.
                || config.width > 3840.
                || config.height > 2160.
            {
                anyhow::bail!("visual QA viewport must be between 1120x720 and 3840x2160");
            }
        }
        Ok(Some(config))
    }
}

actions!(
    cedar,
    [
        FocusResourceSearch,
        RefreshDashboard,
        CloseResourceInspector,
        ToggleCommandPalette,
        ToggleSidebar,
        ToggleShortcutGuide,
        InspectorBack,
        InspectorForward
    ]
);

#[derive(Clone, Copy)]
enum PaletteCommand {
    Navigate(Section),
    Refresh,
    CopyReport,
    ToggleTheme,
}

struct PaletteCommandItem {
    command: PaletteCommand,
    label: String,
    detail: &'static str,
    icon: IconName,
    shortcut: Option<&'static str>,
}

#[derive(Clone, Copy, Default, Eq, PartialEq)]
enum InspectorTab {
    #[default]
    Overview,
    Bindings,
    Observability,
    Audit,
}

#[derive(Clone, Copy, Default, Eq, PartialEq)]
enum ResourceViewMode {
    #[default]
    Table,
    Topology,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
struct WorkspaceState {
    version: u8,
    active_section: String,
    range: String,
    resource_view_mode: String,
    status_filter: String,
    resource_query: String,
    inspector_tab: String,
    selected_resource: Option<String>,
}

impl Default for WorkspaceState {
    fn default() -> Self {
        Self {
            version: 1,
            active_section: "overview".into(),
            range: "24h".into(),
            resource_view_mode: "table".into(),
            status_filter: "all".into(),
            resource_query: String::new(),
            inspector_tab: "overview".into(),
            selected_resource: None,
        }
    }
}

impl WorkspaceState {
    fn sanitized(mut self) -> Self {
        self.version = 1;
        self.active_section = section_name(section_from_name(&self.active_section)).into();
        self.range = match self.range.as_str() {
            "7d" => "7d",
            "30d" => "30d",
            _ => "24h",
        }
        .into();
        self.resource_view_mode = match self.resource_view_mode.as_str() {
            "topology" => "topology",
            _ => "table",
        }
        .into();
        self.status_filter = match self.status_filter.as_str() {
            "healthy" => "healthy",
            "attention" => "attention",
            _ => "all",
        }
        .into();
        self.inspector_tab = match self.inspector_tab.as_str() {
            "bindings" => "bindings",
            "observability" => "observability",
            "audit" => "audit",
            _ => "overview",
        }
        .into();
        self
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
struct PersistedWindowState {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    maximized: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct TopologyNode {
    key: String,
    name: String,
    kind: String,
    status: String,
    x: f32,
    y: f32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TopologyEdge {
    from: usize,
    to: usize,
}

#[derive(Clone, Debug, PartialEq)]
struct TopologyLane {
    label: &'static str,
    x: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct TopologyLayout {
    nodes: Vec<TopologyNode>,
    edges: Vec<TopologyEdge>,
    lanes: Vec<TopologyLane>,
}

#[derive(Clone, Debug)]
struct InvestigationContext {
    finding: AuditFinding,
    resource_keys: Vec<String>,
    cursor: usize,
}

#[derive(Clone)]
struct UiError {
    title: &'static str,
    message: String,
}

#[derive(Clone)]
struct UiNotice {
    message: SharedString,
    error: bool,
}

impl UiError {
    fn new(title: &'static str, message: impl Into<String>) -> Self {
        Self {
            title,
            message: message.into(),
        }
    }
}

impl InvestigationContext {
    fn current_key(&self) -> Option<&str> {
        self.resource_keys.get(self.cursor).map(String::as_str)
    }
}

impl InspectorTab {
    fn from_index(index: usize) -> Self {
        match index {
            1 => Self::Bindings,
            2 => Self::Observability,
            3 => Self::Audit,
            _ => Self::Overview,
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Overview => 0,
            Self::Bindings => 1,
            Self::Observability => 2,
            Self::Audit => 3,
        }
    }
}

struct ResourceTableDelegate {
    owner: WeakEntity<CedarApp>,
    columns: Vec<Column>,
    source_rows: Vec<ResourceRow>,
    rows: Vec<ResourceRow>,
    fingerprint: String,
    sort: Option<(usize, ColumnSort)>,
    workers_only: bool,
    palette: Palette,
    investigation_keys: HashSet<String>,
    investigation_current: Option<String>,
    selected_key: Option<String>,
    filters_active: bool,
    compact: bool,
}

impl ResourceTableDelegate {
    fn new(owner: WeakEntity<CedarApp>, palette: Palette) -> Self {
        Self {
            owner,
            columns: resource_table_columns(false),
            source_rows: Vec::new(),
            rows: Vec::new(),
            fingerprint: String::new(),
            sort: None,
            workers_only: false,
            palette,
            investigation_keys: HashSet::new(),
            investigation_current: None,
            selected_key: None,
            filters_active: false,
            compact: false,
        }
    }

    fn set_compact(&mut self, compact: bool) -> bool {
        if self.compact == compact {
            return false;
        }
        self.compact = compact;
        self.columns = resource_table_columns(compact);
        self.sort = None;
        self.apply_sort();
        true
    }

    fn sync(
        &mut self,
        rows: Vec<ResourceRow>,
        fingerprint: String,
        workers_only: bool,
        palette: Palette,
        investigation_keys: HashSet<String>,
        investigation_current: Option<String>,
    ) -> bool {
        self.palette = palette;
        self.workers_only = workers_only;
        self.investigation_keys = investigation_keys;
        self.investigation_current = investigation_current;
        if self.fingerprint == fingerprint {
            return false;
        }

        self.fingerprint = fingerprint;
        self.source_rows = rows;
        self.apply_sort();
        true
    }

    fn apply_sort(&mut self) {
        self.rows = self.source_rows.clone();
        let Some((column, direction)) = self.sort else {
            return;
        };

        if direction == ColumnSort::Default {
            return;
        }

        let key = self.columns[column].key.clone();
        self.rows.sort_by(|left, right| {
            let ordering = match key.as_ref() {
                "resource" => left.name.cmp(&right.name),
                "kind" => left.kind.cmp(&right.kind),
                "status" => left.status.cmp(&right.status),
                "primary" => left.primary_metric.cmp(&right.primary_metric),
                _ => left.secondary_metric.cmp(&right.secondary_metric),
            };
            if direction == ColumnSort::Descending {
                ordering.reverse()
            } else {
                ordering
            }
        });
    }
}

fn resource_table_columns(compact: bool) -> Vec<Column> {
    let cell_padding = gpui::Edges {
        top: px(4.),
        right: px(12.),
        bottom: px(4.),
        left: px(12.),
    };
    if compact {
        vec![
            Column::new("resource", "RESOURCE")
                .width(px(250.))
                .paddings(cell_padding)
                .fixed_left()
                .sortable(),
            Column::new("status", "STATUS")
                .width(px(110.))
                .paddings(cell_padding)
                .sortable(),
            Column::new("primary", "PRIMARY")
                .width(px(140.))
                .paddings(cell_padding)
                .sortable(),
        ]
    } else {
        vec![
            Column::new("resource", "RESOURCE")
                .width(px(280.))
                .paddings(cell_padding)
                .fixed_left()
                .sortable(),
            Column::new("kind", "KIND")
                .width(px(120.))
                .paddings(cell_padding)
                .sortable(),
            Column::new("status", "STATUS")
                .width(px(130.))
                .paddings(cell_padding)
                .sortable(),
            Column::new("primary", "PRIMARY")
                .width(px(170.))
                .paddings(cell_padding)
                .sortable(),
            Column::new("detail", "DETAIL")
                .width(px(300.))
                .paddings(cell_padding)
                .sortable(),
        ]
    }
}

impl TableDelegate for ResourceTableDelegate {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> &Column {
        &self.columns[col_ix]
    }

    fn render_header(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id("resource-table-header")
            .bg(self.palette.surface.opacity(0.96))
    }

    fn render_th(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .text_size(px(10.))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(self.palette.subtle)
            .child(self.columns[col_ix].name.clone())
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        self.sort = Some((col_ix, sort));
        for (ix, column) in self.columns.iter_mut().enumerate() {
            if column.sort.is_some() {
                column.sort = Some(if ix == col_ix {
                    sort
                } else {
                    ColumnSort::Default
                });
            }
        }
        self.apply_sort();

        let owner = self.owner.clone();
        cx.defer(move |cx| {
            let Some(owner) = owner.upgrade() else {
                return;
            };
            let (table, selected_key) = {
                let app = owner.read(cx);
                (app.resource_table.clone(), app.selected_resource.clone())
            };
            let Some(selected_key) = selected_key else {
                return;
            };
            let selected_row = table
                .read(cx)
                .delegate()
                .rows
                .iter()
                .position(|resource| resource_key(resource) == selected_key);
            if let Some(selected_row) = selected_row {
                table.update(cx, |table, cx| table.set_selected_row(selected_row, cx));
            }
        });
    }

    fn render_tr(
        &mut self,
        row_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> gpui::Stateful<gpui::Div> {
        let key = self.rows.get(row_ix).map(resource_key);
        let selected = key
            .as_ref()
            .is_some_and(|key| self.selected_key.as_ref() == Some(key));
        let current = key
            .as_ref()
            .is_some_and(|key| self.investigation_current.as_ref() == Some(key));
        let related = key
            .as_ref()
            .is_some_and(|key| self.investigation_keys.contains(key));
        div()
            .id(("resource-row", row_ix))
            .group(SharedString::from(format!("resource-row-group-{row_ix}")))
            .cursor_pointer()
            .hover(|row| row.bg(self.palette.hover))
            .active(|row| row.bg(self.palette.selected))
            .when(selected, |row| {
                row.bg(self.palette.selected)
                    .border_l_2()
                    .border_color(self.palette.accent)
            })
            .when(current, |row| row.bg(self.palette.selected))
            .when(related && !current, |row| {
                row.bg(self.palette.accent_soft.opacity(0.42))
            })
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(resource) = self.rows.get(row_ix) else {
            return div().into_any_element();
        };
        let selected = self.selected_key.as_deref() == Some(resource_key(resource).as_str());
        let row_group = SharedString::from(format!("resource-row-group-{row_ix}"));

        match self.columns[col_ix].key.as_ref() {
            "resource" => div()
                .h_full()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .size(px(28.))
                        .flex_none()
                        .rounded(px(7.))
                        .border_1()
                        .border_color(self.palette.border)
                        .bg(self.palette.surface)
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(self.palette.muted)
                        .child(Icon::new(resource_kind_icon(&resource.kind))),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex_grow()
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_ellipsis()
                                .child(resource.name.clone()),
                        )
                        .child(
                            div()
                                .pt(px(2.))
                                .font_family(FONT_MONO)
                                .text_size(px(10.))
                                .line_height(px(14.))
                                .text_color(self.palette.subtle)
                                .text_ellipsis()
                                .child(resource.id.clone()),
                        ),
                )
                .child(
                    div()
                        .ml_auto()
                        .flex_none()
                        .text_color(self.palette.accent)
                        .opacity(if selected { 1. } else { 0. })
                        .group_hover(row_group, |action| action.opacity(1.))
                        .child(Icon::new(IconName::ArrowRight).size_3()),
                )
                .into_any_element(),
            "kind" => div()
                .h_full()
                .flex()
                .items_center()
                .font_family(FONT_MONO)
                .text_size(px(9.))
                .text_color(self.palette.muted)
                .child(resource.kind.to_uppercase())
                .into_any_element(),
            "status" => div()
                .h_full()
                .flex()
                .items_center()
                .child(status_pill(&resource.status, self.palette))
                .into_any_element(),
            "primary" => div()
                .h_full()
                .flex()
                .items_center()
                .font_family(FONT_MONO)
                .text_size(px(12.))
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(resource.primary_metric.clone())
                .into_any_element(),
            _ => div()
                .h_full()
                .flex()
                .items_center()
                .text_size(px(11.))
                .text_color(self.palette.muted)
                .child(resource.secondary_metric.clone())
                .into_any_element(),
        }
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let owner = self.owner.clone();
        empty_resource_state(self.workers_only, self.palette).when(self.filters_active, |view| {
            view.child(
                div().pt_4().child(
                    Button::new("clear-empty-resource-filters")
                        .label("Clear filters")
                        .on_click(move |_, window, cx| {
                            let Some(owner) = owner.upgrade() else {
                                return;
                            };
                            owner.update(cx, |this, cx| {
                                this.status_filter = "all";
                                this.clear_resource_search(window, cx);
                                cx.notify();
                            });
                        }),
                ),
            )
        })
    }
}

fn restored_window_bounds(backend: &Backend, cx: &App) -> WindowBounds {
    let fallback = || WindowBounds::Windowed(Bounds::centered(None, size(px(1440.), px(960.)), cx));
    let Some(saved) = backend
        .preference(WINDOW_STATE_KEY)
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_str::<PersistedWindowState>(&value).ok())
    else {
        return fallback();
    };
    if ![saved.x, saved.y, saved.width, saved.height]
        .into_iter()
        .all(f32::is_finite)
        || saved.width <= 0.
        || saved.height <= 0.
    {
        return fallback();
    }

    let candidate = Bounds::new(
        point(px(saved.x), px(saved.y)),
        size(px(saved.width), px(saved.height)),
    );
    let display = cx
        .displays()
        .into_iter()
        .find(|display| display.bounds().intersects(&candidate))
        .or_else(|| cx.primary_display());
    let Some(display) = display else {
        return fallback();
    };
    let display_bounds = display.bounds();
    let display_x = f32::from(display_bounds.origin.x);
    let display_y = f32::from(display_bounds.origin.y);
    let display_width = f32::from(display_bounds.size.width).max(1.);
    let display_height = f32::from(display_bounds.size.height).max(1.);
    let width = saved
        .width
        .clamp(1120_f32.min(display_width), display_width);
    let height = saved
        .height
        .clamp(720_f32.min(display_height), display_height);
    let x = saved.x.clamp(display_x, display_x + display_width - width);
    let y = saved
        .y
        .clamp(display_y, display_y + display_height - height);
    let bounds = Bounds::new(point(px(x), px(y)), size(px(width), px(height)));
    if saved.maximized {
        WindowBounds::Maximized(bounds)
    } else {
        WindowBounds::Windowed(bounds)
    }
}

pub(crate) fn open_main_window(
    cx: &mut App,
    backend: Arc<Backend>,
    runtime: Arc<Runtime>,
) -> anyhow::Result<()> {
    open_cedar_window(cx, backend, runtime, None)
}

pub(crate) fn open_visual_qa_window(
    cx: &mut App,
    backend: Arc<Backend>,
    runtime: Arc<Runtime>,
    config: VisualQaConfig,
) -> anyhow::Result<()> {
    open_cedar_window(cx, backend, runtime, Some(config))
}

fn open_cedar_window(
    cx: &mut App,
    backend: Arc<Backend>,
    runtime: Arc<Runtime>,
    visual_qa: Option<VisualQaConfig>,
) -> anyhow::Result<()> {
    cx.bind_keys([
        KeyBinding::new("cmd-f", FocusResourceSearch, None),
        KeyBinding::new("ctrl-f", FocusResourceSearch, None),
        KeyBinding::new("cmd-r", RefreshDashboard, None),
        KeyBinding::new("ctrl-r", RefreshDashboard, None),
        KeyBinding::new("escape", CloseResourceInspector, None),
        KeyBinding::new("escape", CloseResourceInspector, Some("Table")),
        KeyBinding::new("cmd-k", ToggleCommandPalette, None),
        KeyBinding::new("ctrl-k", ToggleCommandPalette, None),
        KeyBinding::new("cmd-b", ToggleSidebar, None),
        KeyBinding::new("ctrl-b", ToggleSidebar, None),
        KeyBinding::new("?", ToggleShortcutGuide, None),
        KeyBinding::new("alt-left", InspectorBack, None),
        KeyBinding::new("alt-right", InspectorForward, None),
    ]);
    let bounds = visual_qa.map_or_else(
        || restored_window_bounds(&backend, cx),
        |config| {
            WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(config.width), px(config.height)),
                cx,
            ))
        },
    );
    let mut titlebar = TitleBar::title_bar_options();
    titlebar.title = Some(
        visual_qa
            .map(|config| format!("Cedar Visual QA — {}", config.scenario.name()))
            .unwrap_or_else(|| "Cedar".into())
            .into(),
    );
    cx.open_window(
        WindowOptions {
            window_bounds: Some(bounds),
            window_min_size: Some(size(px(1120.), px(720.))),
            app_id: Some("dev.oshtz.cedar".into()),
            titlebar: Some(titlebar),
            ..Default::default()
        },
        |window, cx| {
            let view = cx.new(|cx| CedarApp::new(backend, runtime, visual_qa, window, cx));
            cx.new(|cx| Root::new(view, window, cx))
        },
    )?;
    Ok(())
}

struct CedarApp {
    backend: Arc<Backend>,
    runtime: Arc<Runtime>,
    visual_qa: bool,
    connection: Option<ConnectionState>,
    snapshot: DashboardSnapshot,
    accounts: Vec<Account>,
    selected_account_id: Option<String>,
    active_section: Section,
    range: &'static str,
    syncing: bool,
    error: Option<UiError>,
    report_copied: bool,
    selected_resource: Option<String>,
    selected_finding: Option<AuditFinding>,
    investigation: Option<InvestigationContext>,
    status_filter: &'static str,
    token_input: Entity<InputState>,
    search_input: Entity<InputState>,
    command_input: Entity<InputState>,
    command_palette_open: bool,
    command_palette_return_focus: Option<FocusHandle>,
    command_palette_scroll: ScrollHandle,
    shortcut_guide_open: bool,
    shortcut_guide_return_focus: Option<FocusHandle>,
    shortcut_guide_focus: FocusHandle,
    investigation_focus: FocusHandle,
    palette_selected: usize,
    resource_table: Entity<TableState<ResourceTableDelegate>>,
    resource_tree: Entity<TreeState>,
    resource_tree_fingerprint: String,
    resource_view_mode: ResourceViewMode,
    topology_scale: f32,
    topology_offset_x: f32,
    topology_offset_y: f32,
    topology_auto_fit: bool,
    topology_drag: Option<(Point<Pixels>, f32, f32)>,
    topology_layout: Arc<TopologyLayout>,
    topology_fingerprint: String,
    inspector_split: Entity<ResizableState>,
    inspector_width: f32,
    inspector_resize_generation: u64,
    inspector_tab: InspectorTab,
    inspector_history: Vec<String>,
    inspector_history_index: Option<usize>,
    worker_preferences: WorkerAuditPreferences,
    recent_changes: Vec<SnapshotChange>,
    sidebar_collapsed: bool,
    dark: bool,
    reduced_motion: bool,
    automatic_update_checks: bool,
    update_status: UpdateStatus,
    workspace_state_fingerprint: String,
    workspace_save_generation: u64,
    window_save_generation: u64,
    disconnect_confirmation_open: bool,
    notice: Option<UiNotice>,
    _subscriptions: Vec<Subscription>,
}

impl CedarApp {
    fn new(
        backend: Arc<Backend>,
        runtime: Arc<Runtime>,
        visual_qa: Option<VisualQaConfig>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let visual_qa_enabled = visual_qa.is_some();
        let workspace_state = if visual_qa_enabled {
            WorkspaceState::default()
        } else {
            backend
                .preference(WORKSPACE_STATE_KEY)
                .ok()
                .flatten()
                .and_then(|value| serde_json::from_str::<WorkspaceState>(&value).ok())
                .unwrap_or_default()
                .sanitized()
        };
        let token_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Cloudflare API token")
                .masked(true)
        });
        let resource_query = workspace_state.resource_query.clone();
        let search_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Search resources…")
                .default_value(resource_query)
        });
        let command_input = cx.new(|cx| InputState::new(window, cx).placeholder("Type a command…"));
        let owner = cx.entity().downgrade();
        let resource_table = cx.new(|cx| {
            TableState::new(
                ResourceTableDelegate::new(owner, Palette::light()),
                window,
                cx,
            )
            .col_selectable(false)
            .col_movable(false)
            .loop_selection(false)
        });
        let resource_tree = cx.new(|cx| TreeState::new(cx));
        let inspector_split = cx.new(|_| ResizableState::default());
        let token_subscription = cx.subscribe(&token_input, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.accounts.clear();
                this.selected_account_id = None;
                this.error = None;
                cx.notify();
            }
        });
        let search_subscription = cx.subscribe(&search_input, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        let command_subscription =
            cx.subscribe(&command_input, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.palette_selected = 0;
                    this.command_palette_scroll.scroll_to_item(0);
                    cx.notify();
                }
            });
        let table_subscription =
            cx.subscribe(&resource_table, |this, table, event: &TableEvent, cx| {
                if let TableEvent::SelectRow(row_ix) = event
                    && let Some(resource) = table.read(cx).delegate().rows.get(*row_ix)
                {
                    let key = resource_key(resource);
                    this.open_resource(key, true, cx);
                }
            });
        let window_bounds_subscription = (!visual_qa_enabled).then(|| {
            cx.observe_window_bounds(window, |this, window, cx| {
                this.schedule_window_state_save(window, cx);
            })
        });
        let dark = visual_qa.map_or_else(
            || backend.preference(THEME_KEY).ok().flatten().as_deref() == Some("dark"),
            |config| config.dark,
        );
        let sidebar_collapsed = !visual_qa_enabled
            && backend
                .preference(SIDEBAR_COLLAPSED_KEY)
                .ok()
                .flatten()
                .as_deref()
                == Some("true");
        let reduced_motion = visual_qa_enabled
            || backend
                .preference(REDUCED_MOTION_KEY)
                .ok()
                .flatten()
                .as_deref()
                == Some("true");
        let automatic_update_checks = visual_qa_enabled
            || backend
                .preference(AUTOMATIC_UPDATE_CHECKS_KEY)
                .ok()
                .flatten()
                .as_deref()
                != Some("false");
        let recovery_error = (!visual_qa_enabled)
            .then(updater::take_recovery_error)
            .flatten();
        let worker_preferences = if visual_qa_enabled {
            WorkerAuditPreferences::default()
        } else {
            backend
                .preference(WORKER_PREFERENCES_KEY)
                .ok()
                .flatten()
                .and_then(|value| serde_json::from_str(&value).ok())
                .unwrap_or_default()
        };
        let inspector_width = if visual_qa_enabled {
            380.
        } else {
            backend
                .preference(INSPECTOR_WIDTH_KEY)
                .ok()
                .flatten()
                .and_then(|value| value.parse::<f32>().ok())
                .unwrap_or(380.)
                .clamp(320., 560.)
        };

        Theme::change(
            if dark {
                ThemeMode::Dark
            } else {
                ThemeMode::Light
            },
            Some(window),
            cx,
        );
        apply_component_theme(dark, cx);

        let active_section = section_from_name(&workspace_state.active_section);
        let range = match workspace_state.range.as_str() {
            "7d" => "7d",
            "30d" => "30d",
            _ => "24h",
        };
        let status_filter = match workspace_state.status_filter.as_str() {
            "healthy" => "healthy",
            "attention" => "attention",
            _ => "all",
        };
        let resource_view_mode = if workspace_state.resource_view_mode == "topology" {
            ResourceViewMode::Topology
        } else {
            ResourceViewMode::Table
        };
        let inspector_tab = inspector_tab_from_name(&workspace_state.inspector_tab);
        let selected_resource = workspace_state.selected_resource.clone();
        let workspace_state_fingerprint =
            serde_json::to_string(&workspace_state).unwrap_or_default();
        let inspector_history = selected_resource.iter().cloned().collect::<Vec<_>>();
        let inspector_history_index = (!inspector_history.is_empty()).then_some(0);

        let mut app = Self {
            backend,
            runtime,
            visual_qa: visual_qa_enabled,
            connection: None,
            snapshot: empty_snapshot(),
            accounts: Vec::new(),
            selected_account_id: None,
            active_section,
            range,
            syncing: true,
            error: None,
            report_copied: false,
            selected_resource,
            selected_finding: None,
            investigation: None,
            status_filter,
            token_input,
            search_input,
            command_input,
            command_palette_open: false,
            command_palette_return_focus: None,
            command_palette_scroll: ScrollHandle::new(),
            shortcut_guide_open: false,
            shortcut_guide_return_focus: None,
            shortcut_guide_focus: cx.focus_handle(),
            investigation_focus: cx.focus_handle(),
            palette_selected: 0,
            resource_table,
            resource_tree,
            resource_tree_fingerprint: String::new(),
            resource_view_mode,
            topology_scale: 1.,
            topology_offset_x: 0.,
            topology_offset_y: 0.,
            topology_auto_fit: true,
            topology_drag: None,
            topology_layout: Arc::new(TopologyLayout::default()),
            topology_fingerprint: String::new(),
            inspector_split,
            inspector_width,
            inspector_resize_generation: 0,
            inspector_tab,
            inspector_history,
            inspector_history_index,
            worker_preferences,
            recent_changes: Vec::new(),
            sidebar_collapsed,
            dark,
            reduced_motion,
            automatic_update_checks,
            update_status: recovery_error
                .clone()
                .map(UpdateStatus::Error)
                .unwrap_or_default(),
            workspace_state_fingerprint,
            workspace_save_generation: 0,
            window_save_generation: 0,
            disconnect_confirmation_open: false,
            notice: None,
            _subscriptions: vec![
                token_subscription,
                search_subscription,
                command_subscription,
                table_subscription,
            ]
            .into_iter()
            .chain(window_bounds_subscription)
            .collect(),
        };
        if let Some(config) = visual_qa {
            app.apply_visual_qa(config, window, cx);
        } else {
            app.load_initial(cx);
            if app.automatic_update_checks
                && recovery_error.is_none()
                && !cfg!(debug_assertions)
                && updater::supported()
            {
                app.check_for_updates(cx);
            }
        }
        app
    }

    fn apply_visual_qa(
        &mut self,
        config: VisualQaConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let snapshot = visual_qa_snapshot();
        let account = snapshot.account.clone().expect("visual QA account");
        self.connection = Some(ConnectionState {
            configured: true,
            account: Some(account.clone()),
            token_present: true,
            storage: "os-keychain",
        });
        self.accounts = vec![account.clone()];
        self.selected_account_id = Some(account.id);
        self.snapshot = snapshot;
        self.range = "24h";
        self.syncing = false;
        self.error = None;
        self.selected_resource = None;
        self.selected_finding = None;
        self.investigation = None;
        self.status_filter = "all";
        self.resource_view_mode = ResourceViewMode::Table;
        self.inspector_tab = InspectorTab::Overview;
        self.inspector_history.clear();
        self.inspector_history_index = None;
        self.active_section = Section::Overview;

        match config.scenario {
            VisualQaScenario::Audit => {}
            VisualQaScenario::ResourcesTable => self.active_section = Section::Resources,
            VisualQaScenario::ResourcesTopology => {
                self.active_section = Section::Resources;
                self.resource_view_mode = ResourceViewMode::Topology;
            }
            VisualQaScenario::WorkersInspector => {
                self.active_section = Section::Workers;
                self.open_resource("worker-artor-api".into(), true, cx);
                self.inspector_tab = InspectorTab::Observability;
            }
            VisualQaScenario::Cost => self.active_section = Section::Billing,
            VisualQaScenario::Connection => self.active_section = Section::Connection,
            VisualQaScenario::Settings => self.active_section = Section::Settings,
            VisualQaScenario::CommandPalette => {
                self.set_command_palette_open(true, window, cx);
            }
            VisualQaScenario::Shortcuts => {
                self.set_shortcut_guide_open(true, window, cx);
            }
            VisualQaScenario::EmptyResources => {
                self.active_section = Section::Resources;
                self.snapshot.resources.clear();
                self.snapshot.inventory = Default::default();
            }
            VisualQaScenario::Loading => {
                self.snapshot = empty_snapshot();
                self.syncing = true;
            }
            VisualQaScenario::Error => {
                self.error = Some(UiError::new(
                    "Collector sync needs attention",
                    "Workers analytics returned a scoped response. Cedar retained the last complete snapshot.",
                ));
            }
        }
        cx.notify();
    }

    fn load_initial(&mut self, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        let range = self.range;
        let task = self.runtime.spawn(async move {
            let connection = backend
                .get_connection()
                .map_err(|error| error.to_string())?;
            let snapshot = if connection.configured {
                backend
                    .get_cached_snapshot(range)
                    .map_err(|error| error.to_string())?
            } else {
                None
            };
            Ok::<_, String>((connection, snapshot))
        });
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .map_err(|error| error.to_string())
                .and_then(|value| value);
            if let Some(this) = this.upgrade() {
                let _ = this.update(cx, |this, cx| {
                    this.syncing = false;
                    match result {
                        Ok((connection, snapshot)) => {
                            this.connection = Some(connection);
                            if let Some(snapshot) = snapshot {
                                this.snapshot = snapshot;
                                this.reconcile_selected_resource();
                            }
                            if this.connected() {
                                this.refresh(false, cx);
                            }
                        }
                        Err(error) => {
                            this.error = Some(UiError::new("Cedar could not load", error))
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn refresh(&mut self, force: bool, cx: &mut Context<Self>) {
        if self.syncing
            || !self
                .connection
                .as_ref()
                .is_some_and(|connection| connection.configured)
        {
            return;
        }
        self.syncing = true;
        self.error = None;
        cx.notify();
        let backend = self.backend.clone();
        let range = self.range;
        let previous = self.snapshot.clone();
        let task = self.runtime.spawn(async move {
            backend
                .sync_cloudflare(range, force)
                .await
                .map_err(|error| error.to_string())
        });
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .map_err(|error| error.to_string())
                .and_then(|value| value);
            if let Some(this) = this.upgrade() {
                let _ = this.update(cx, |this, cx| {
                    this.syncing = false;
                    match result {
                        Ok(snapshot) => {
                            this.recent_changes = diff_snapshots(&previous, &snapshot);
                            this.snapshot = snapshot;
                            this.reconcile_selected_resource();
                            this.report_copied = false;
                            this.selected_finding = None;
                            this.investigation = None;
                        }
                        Err(error) => {
                            this.error = Some(UiError::new("Sync could not complete", error))
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn discover_accounts(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        let token = self.token_input.read(cx).unmask_value().to_string();
        if token.trim().is_empty() || self.syncing {
            return;
        }
        self.syncing = true;
        self.error = None;
        cx.notify();
        let backend = self.backend.clone();
        let task = self.runtime.spawn(async move {
            backend
                .discover_accounts(&token)
                .await
                .map_err(|error| error.to_string())
        });
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .map_err(|error| error.to_string())
                .and_then(|value| value);
            if let Some(this) = this.upgrade() {
                let _ = this.update(cx, |this, cx| {
                    this.syncing = false;
                    match result {
                        Ok(accounts) => {
                            this.selected_account_id =
                                accounts.first().map(|account| account.id.clone());
                            this.accounts = accounts;
                        }
                        Err(error) => {
                            this.error = Some(UiError::new("Token verification failed", error))
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn connect(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        let token = self.token_input.read(cx).unmask_value().to_string();
        if token.trim().is_empty() || self.syncing {
            return;
        }
        self.syncing = true;
        self.error = None;
        cx.notify();
        let backend = self.backend.clone();
        let account_id = self.selected_account_id.clone();
        let task = self.runtime.spawn(async move {
            backend
                .connect_cloudflare(&token, account_id.as_deref())
                .await
                .map_err(|error| error.to_string())
        });
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .map_err(|error| error.to_string())
                .and_then(|value| value);
            if let Some(this) = this.upgrade() {
                let _ = this.update(cx, |this, cx| {
                    this.syncing = false;
                    match result {
                        Ok(result) => {
                            this.accounts = result.accounts;
                            this.connection = result.connection;
                            if let Some(snapshot) = result.snapshot {
                                this.snapshot = snapshot;
                                this.reconcile_selected_resource();
                            }
                            this.report_copied = false;
                            this.selected_finding = None;
                            this.investigation = None;
                            this.active_section = Section::Overview;
                        }
                        Err(error) => this.error = Some(UiError::new("Connection failed", error)),
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn clear_connection(
        &mut self,
        _: &gpui::ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = self.backend.clear_connection() {
            self.error = Some(UiError::new("Disconnect failed", error.to_string()));
            self.show_notice("Cedar could not disconnect the account", true, window, cx);
            return;
        }
        self.disconnect_confirmation_open = false;
        self.connection = None;
        self.snapshot = empty_snapshot();
        self.accounts.clear();
        self.selected_account_id = None;
        self.selected_resource = None;
        self.selected_finding = None;
        self.investigation = None;
        self.report_copied = false;
        self.active_section = Section::Connection;
        self.show_notice("Cloudflare account disconnected", false, window, cx);
        cx.notify();
    }

    fn toggle_disconnect_confirmation(
        &mut self,
        _: &gpui::ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.disconnect_confirmation_open = !self.disconnect_confirmation_open;
        cx.notify();
    }

    fn show_notice(
        &mut self,
        message: &'static str,
        error: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.reduced_motion {
            self.notice = Some(UiNotice {
                message: message.into(),
                error,
            });
        } else if error {
            window.push_notification(Notification::error(message), cx);
        } else {
            window.push_notification(Notification::success(message), cx);
        }
        cx.notify();
    }

    fn dismiss_notice(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.notice = None;
        cx.notify();
    }

    fn toggle_theme(&mut self, _: &gpui::ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.dark = !self.dark;
        Theme::change(
            if self.dark {
                ThemeMode::Dark
            } else {
                ThemeMode::Light
            },
            Some(window),
            cx,
        );
        apply_component_theme(self.dark, cx);
        match self
            .backend
            .set_preference(THEME_KEY, if self.dark { "dark" } else { "light" })
        {
            Ok(()) => self.show_notice(
                if self.dark {
                    "Dark theme saved"
                } else {
                    "Light theme saved"
                },
                false,
                window,
                cx,
            ),
            Err(error) => {
                self.error = Some(UiError::new(
                    "Theme preference was not saved",
                    error.to_string(),
                ));
                self.show_notice("Theme changed for this session only", true, window, cx);
            }
        }
        cx.notify();
    }

    fn toggle_reduced_motion(
        &mut self,
        _: &gpui::ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reduced_motion = !self.reduced_motion;
        let result = self.backend.set_preference(
            REDUCED_MOTION_KEY,
            if self.reduced_motion { "true" } else { "false" },
        );
        match result {
            Ok(()) => self.show_notice(
                if self.reduced_motion {
                    "Reduced motion saved"
                } else {
                    "Full motion saved"
                },
                false,
                window,
                cx,
            ),
            Err(error) => {
                self.error = Some(UiError::new(
                    "Motion preference was not saved",
                    error.to_string(),
                ));
                self.show_notice("Motion changed for this session only", true, window, cx);
            }
        }
        cx.notify();
    }

    fn toggle_automatic_update_checks(
        &mut self,
        _: &gpui::ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.automatic_update_checks = !self.automatic_update_checks;
        match self.backend.set_preference(
            AUTOMATIC_UPDATE_CHECKS_KEY,
            if self.automatic_update_checks {
                "true"
            } else {
                "false"
            },
        ) {
            Ok(()) => self.show_notice(
                if self.automatic_update_checks {
                    "Automatic update checks enabled"
                } else {
                    "Automatic update checks disabled"
                },
                false,
                window,
                cx,
            ),
            Err(error) => {
                self.error = Some(UiError::new(
                    "Update preference was not saved",
                    error.to_string(),
                ));
                self.show_notice(
                    "Update preference changed for this session only",
                    true,
                    window,
                    cx,
                );
            }
        }
        if self.automatic_update_checks && matches!(self.update_status, UpdateStatus::Idle) {
            self.check_for_updates(cx);
        }
        cx.notify();
    }

    fn check_for_updates(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.update_status,
            UpdateStatus::Checking | UpdateStatus::Downloading(_) | UpdateStatus::Installing
        ) {
            return;
        }
        self.update_status = UpdateStatus::Checking;
        cx.notify();
        let task = self.runtime.spawn(updater::check_for_update());
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .map_err(|error| error.to_string())
                .and_then(|value| value.map_err(|error| error.to_string()));
            if let Some(this) = this.upgrade() {
                let _ = this.update(cx, |this, cx| {
                    this.update_status = match result {
                        Ok(Some(update)) => UpdateStatus::Available(update),
                        Ok(None) => UpdateStatus::UpToDate,
                        Err(error) => UpdateStatus::Error(error),
                    };
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn check_for_updates_click(
        &mut self,
        _: &gpui::ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.check_for_updates(cx);
    }

    fn download_update_click(
        &mut self,
        _: &gpui::ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let UpdateStatus::Available(update) = self.update_status.clone() else {
            return;
        };
        self.update_status = UpdateStatus::Downloading(update.clone());
        cx.notify();
        let task = self.runtime.spawn(updater::download_update(update));
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .map_err(|error| error.to_string())
                .and_then(|value| value.map_err(|error| error.to_string()));
            if let Some(this) = this.upgrade() {
                let _ = this.update(cx, |this, cx| {
                    this.update_status = match result {
                        Ok(download) => UpdateStatus::Ready(download),
                        Err(error) => UpdateStatus::Error(error),
                    };
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn install_update_click(
        &mut self,
        _: &gpui::ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let UpdateStatus::Ready(download) = self.update_status.clone() else {
            return;
        };
        self.update_status = UpdateStatus::Installing;
        cx.notify();
        match updater::stage_install(&download) {
            Ok(()) => cx.quit(),
            Err(error) => {
                self.update_status = UpdateStatus::Error(error.to_string());
                cx.notify();
            }
        }
    }

    fn set_range(&mut self, range: &'static str, cx: &mut Context<Self>) {
        if self.range == range {
            return;
        }
        self.range = range;
        self.snapshot.range = range.into();
        self.report_copied = false;
        self.selected_finding = None;
        self.investigation = None;
        self.refresh(false, cx);
    }

    fn copy_report(&mut self, _: &gpui::ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let findings = build_audit_findings(&self.snapshot, &self.worker_preferences);
        let report = build_audit_report(&self.snapshot, &findings, &self.recent_changes);
        cx.write_to_clipboard(ClipboardItem::new_string(report));
        self.report_copied = true;
        self.show_notice("Audit report copied", false, window, cx);
        cx.notify();
    }

    fn set_worker_preference(&mut self, preference: WorkerAuditPreference, cx: &mut Context<Self>) {
        let Some(key) = self.selected_resource.clone() else {
            return;
        };
        if preference == WorkerAuditPreference::Normal {
            self.worker_preferences.remove(&key);
        } else {
            self.worker_preferences.insert(key, preference);
        }
        if let Ok(value) = serde_json::to_string(&self.worker_preferences)
            && let Err(error) = self.backend.set_preference(WORKER_PREFERENCES_KEY, &value)
        {
            self.error = Some(UiError::new(
                "Worker preference was not saved",
                error.to_string(),
            ));
        }
        cx.notify();
    }

    fn connected(&self) -> bool {
        self.connection
            .as_ref()
            .is_some_and(|connection| connection.configured)
    }

    fn toggle_sidebar_state(&mut self, cx: &mut Context<Self>) {
        self.sidebar_collapsed = !self.sidebar_collapsed;
        if let Err(error) = self.backend.set_preference(
            SIDEBAR_COLLAPSED_KEY,
            if self.sidebar_collapsed {
                "true"
            } else {
                "false"
            },
        ) {
            self.error = Some(UiError::new(
                "Sidebar preference was not saved",
                error.to_string(),
            ));
        }
        cx.notify();
    }

    fn toggle_sidebar_click(
        &mut self,
        _: &gpui::ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_sidebar_state(cx);
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.toggle_sidebar_state(cx);
    }

    fn resize_inspector(&mut self, width: f32, cx: &mut Context<Self>) {
        self.inspector_width = width.clamp(320., 560.);
        self.inspector_resize_generation = self.inspector_resize_generation.wrapping_add(1);
        let generation = self.inspector_resize_generation;
        let timer = self.runtime.spawn(async {
            tokio::time::sleep(Duration::from_millis(300)).await;
        });

        cx.spawn(async move |this, cx| {
            let _ = timer.await;
            let Some(this) = this.upgrade() else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                if this.inspector_resize_generation != generation {
                    return;
                }
                if let Err(error) = this
                    .backend
                    .set_preference(INSPECTOR_WIDTH_KEY, &format!("{:.0}", this.inspector_width))
                {
                    this.error = Some(UiError::new(
                        "Inspector width was not saved",
                        error.to_string(),
                    ));
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn workspace_state(&self, cx: &App) -> WorkspaceState {
        WorkspaceState {
            version: 1,
            active_section: section_name(self.active_section).into(),
            range: self.range.into(),
            resource_view_mode: match self.resource_view_mode {
                ResourceViewMode::Table => "table",
                ResourceViewMode::Topology => "topology",
            }
            .into(),
            status_filter: self.status_filter.into(),
            resource_query: self.search_input.read(cx).value().to_string(),
            inspector_tab: inspector_tab_name(self.inspector_tab).into(),
            selected_resource: self.selected_resource.clone(),
        }
    }

    fn persist_workspace_if_changed(&mut self, cx: &mut Context<Self>) {
        if self.visual_qa {
            return;
        }
        let Ok(value) = serde_json::to_string(&self.workspace_state(cx)) else {
            return;
        };
        if value == self.workspace_state_fingerprint {
            return;
        }
        self.workspace_state_fingerprint = value.clone();
        self.workspace_save_generation = self.workspace_save_generation.wrapping_add(1);
        let generation = self.workspace_save_generation;
        let timer = self.runtime.spawn(async {
            tokio::time::sleep(Duration::from_millis(240)).await;
        });

        cx.spawn(async move |this, cx| {
            let _ = timer.await;
            let Some(this) = this.upgrade() else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                if this.workspace_save_generation != generation {
                    return;
                }
                if let Err(error) = this.backend.set_preference(WORKSPACE_STATE_KEY, &value) {
                    this.error = Some(UiError::new(
                        "Workspace state was not saved",
                        error.to_string(),
                    ));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn schedule_window_state_save(&mut self, window: &Window, cx: &mut Context<Self>) {
        let bounds = window.window_bounds().get_bounds();
        let state = PersistedWindowState {
            x: f32::from(bounds.origin.x),
            y: f32::from(bounds.origin.y),
            width: f32::from(bounds.size.width),
            height: f32::from(bounds.size.height),
            maximized: window.is_maximized(),
        };
        let Ok(value) = serde_json::to_string(&state) else {
            return;
        };
        self.window_save_generation = self.window_save_generation.wrapping_add(1);
        let generation = self.window_save_generation;
        let timer = self.runtime.spawn(async {
            tokio::time::sleep(Duration::from_millis(300)).await;
        });

        cx.spawn(async move |this, cx| {
            let _ = timer.await;
            let Some(this) = this.upgrade() else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                if this.window_save_generation != generation {
                    return;
                }
                if let Err(error) = this.backend.set_preference(WINDOW_STATE_KEY, &value) {
                    this.error = Some(UiError::new(
                        "Window position was not saved",
                        error.to_string(),
                    ));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn reconcile_selected_resource(&mut self) {
        let Some(key) = self.selected_resource.as_deref() else {
            return;
        };
        if !self
            .snapshot
            .resources
            .iter()
            .any(|resource| resource_key(resource) == key)
        {
            self.selected_resource = None;
            self.inspector_history.clear();
            self.inspector_history_index = None;
        }
    }

    fn open_resource(&mut self, key: String, record_history: bool, cx: &mut Context<Self>) {
        if let Some(investigation) = self.investigation.as_mut()
            && let Some(index) = investigation
                .resource_keys
                .iter()
                .position(|candidate| candidate == &key)
        {
            investigation.cursor = index;
        }
        if self.selected_resource.as_deref() == Some(&key) {
            return;
        }

        if record_history {
            record_inspector_history(
                &mut self.inspector_history,
                &mut self.inspector_history_index,
                &key,
            );
        }

        self.selected_resource = Some(key);
        self.selected_finding = None;
        self.inspector_tab = InspectorTab::Overview;
        cx.notify();
    }

    fn clear_resource_search(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_input
            .update(cx, |input, cx| input.set_value("", window, cx));
    }

    fn start_investigation(
        &mut self,
        finding: AuditFinding,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let resource_keys = finding_resource_keys(&finding, &self.snapshot.resources);
        self.investigation = Some(InvestigationContext {
            finding: finding.clone(),
            resource_keys,
            cursor: 0,
        });
        self.selected_finding = None;
        self.selected_resource = None;
        self.status_filter = "all";
        self.clear_resource_search(window, cx);
        self.resource_table
            .update(cx, |table, cx| table.clear_selection(cx));
        self.investigation_focus.focus(window);
        cx.notify();
    }

    fn open_finding_details(
        &mut self,
        finding: AuditFinding,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.start_investigation(finding.clone(), window, cx);
        self.selected_finding = Some(finding);
        cx.notify();
    }

    fn finish_investigation(&mut self, cx: &mut Context<Self>) {
        self.investigation = None;
        self.selected_finding = None;
        self.selected_resource = None;
        self.resource_table
            .update(cx, |table, cx| table.clear_selection(cx));
        cx.notify();
    }

    fn current_investigation_resource(&self) -> Option<&ResourceRow> {
        let key = self.investigation.as_ref()?.current_key()?;
        self.snapshot
            .resources
            .iter()
            .find(|resource| resource_key(resource) == key)
    }

    fn set_investigation_cursor(&mut self, direction: isize, cx: &mut Context<Self>) {
        let Some(investigation) = self.investigation.as_mut() else {
            return;
        };
        let len = investigation.resource_keys.len();
        if len == 0 {
            return;
        }
        investigation.cursor =
            (investigation.cursor as isize + direction).rem_euclid(len as isize) as usize;
        let key = investigation.resource_keys[investigation.cursor].clone();
        self.selected_finding = None;
        self.selected_resource = None;
        if let Some(resource) = self
            .snapshot
            .resources
            .iter()
            .find(|resource| resource_key(resource) == key)
        {
            self.active_section = section_for_resource(resource);
        }
        self.resource_table
            .update(cx, |table, cx| table.clear_selection(cx));
        cx.notify();
    }

    fn open_current_investigation_resource(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self
            .investigation
            .as_ref()
            .and_then(InvestigationContext::current_key)
            .map(str::to_owned)
        else {
            return;
        };
        if let Some(resource) = self
            .snapshot
            .resources
            .iter()
            .find(|resource| resource_key(resource) == key)
        {
            self.active_section = section_for_resource(resource);
        }
        self.open_resource(key, true, cx);
        self.sync_resource_table_selection(cx);
    }

    fn focus_investigation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.selected_finding = None;
        self.selected_resource = None;
        self.status_filter = "all";
        self.clear_resource_search(window, cx);
        if let Some((section, use_topology)) = self
            .current_investigation_resource()
            .map(|resource| (section_for_resource(resource), resource.kind != "worker"))
        {
            self.active_section = section;
            if use_topology {
                self.resource_view_mode = ResourceViewMode::Topology;
            }
        } else if let Some(section) = self
            .investigation
            .as_ref()
            .and_then(|investigation| investigation.finding.section)
        {
            self.active_section = section;
        }
        self.resource_table
            .update(cx, |table, cx| table.clear_selection(cx));
        cx.notify();
    }

    fn open_investigation_resource(
        &mut self,
        key: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(investigation) = self.investigation.as_mut()
            && let Some(index) = investigation
                .resource_keys
                .iter()
                .position(|candidate| candidate == &key)
        {
            investigation.cursor = index;
        }
        self.status_filter = "all";
        self.clear_resource_search(window, cx);
        if let Some(resource) = self
            .snapshot
            .resources
            .iter()
            .find(|resource| resource_key(resource) == key)
        {
            self.active_section = section_for_resource(resource);
        }
        self.open_resource(key, true, cx);
        self.sync_resource_table_selection(cx);
    }

    fn sync_resource_table_selection(&self, cx: &mut Context<Self>) {
        let selected_key = self.selected_resource.clone();
        self.resource_table.update(cx, |table, cx| {
            let selected_row = selected_key.as_ref().and_then(|key| {
                table
                    .delegate()
                    .rows
                    .iter()
                    .position(|resource| resource_key(resource) == *key)
            });
            if table.selected_row() == selected_row {
                return;
            }
            if let Some(row) = selected_row {
                table.set_selected_row(row, cx);
            } else {
                table.clear_selection(cx);
            }
        });
    }

    fn inspector_history_target(&self, direction: isize) -> Option<usize> {
        find_inspector_history_target(
            &self.inspector_history,
            self.inspector_history_index,
            direction,
            |key| {
                self.snapshot
                    .resources
                    .iter()
                    .any(|resource| resource_key(resource) == key)
            },
        )
    }

    fn navigate_inspector_history(&mut self, direction: isize, cx: &mut Context<Self>) {
        let Some(index) = self.inspector_history_target(direction) else {
            return;
        };
        let key = self.inspector_history[index].clone();
        self.inspector_history_index = Some(index);
        self.open_resource(key, false, cx);
        self.sync_resource_table_selection(cx);
    }

    fn inspector_back(&mut self, _: &InspectorBack, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_finding.is_some() {
            return;
        }
        self.navigate_inspector_history(-1, cx);
    }

    fn inspector_forward(&mut self, _: &InspectorForward, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_finding.is_some() {
            return;
        }
        self.navigate_inspector_history(1, cx);
    }

    fn focus_resource_search(
        &mut self,
        _: &FocusResourceSearch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(self.active_section, Section::Resources | Section::Workers) {
            self.active_section = Section::Resources;
        }
        self.selected_finding = None;
        self.search_input
            .update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    fn refresh_dashboard(&mut self, _: &RefreshDashboard, _: &mut Window, cx: &mut Context<Self>) {
        self.refresh(true, cx);
    }

    fn close_resource_inspector(
        &mut self,
        _: &CloseResourceInspector,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.command_palette_open || self.shortcut_guide_open {
            cx.propagate();
            return;
        }
        let closed_resource = self.selected_resource.take().is_some();
        let closed_finding = self.selected_finding.take().is_some();
        if closed_resource || closed_finding {
            self.resource_table
                .update(cx, |table, cx| table.clear_selection(cx));
            cx.notify();
        } else if self.investigation.is_some() {
            self.finish_investigation(cx);
        } else {
            cx.propagate();
        }
    }

    fn toggle_command_palette(
        &mut self,
        _: &ToggleCommandPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_command_palette_open(!self.command_palette_open, window, cx);
    }

    fn toggle_command_palette_click(
        &mut self,
        _: &gpui::ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_command_palette_open(!self.command_palette_open, window, cx);
    }

    fn set_command_palette_open(
        &mut self,
        open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if open && self.shortcut_guide_open {
            self.shortcut_guide_open = false;
            if let Some(focus) = self.shortcut_guide_return_focus.take() {
                focus.focus(window);
            }
        }
        if open == self.command_palette_open {
            return;
        }
        self.command_palette_open = open;
        self.palette_selected = 0;
        if open {
            self.command_palette_return_focus = window.focused(cx);
            self.command_palette_scroll.scroll_to_item(0);
            self.command_input.update(cx, |input, cx| {
                input.set_value("", window, cx);
                input.focus(window, cx);
            });
        } else if let Some(focus) = self.command_palette_return_focus.take() {
            focus.focus(window);
        } else if self.investigation.is_some() {
            self.investigation_focus.focus(window);
        }
        cx.notify();
    }

    fn toggle_shortcut_guide(
        &mut self,
        _: &ToggleShortcutGuide,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.shortcut_guide_open
            && (self.token_input.focus_handle(cx).is_focused(window)
                || self.search_input.focus_handle(cx).is_focused(window)
                || self.command_input.focus_handle(cx).is_focused(window))
        {
            cx.propagate();
            return;
        }
        self.set_shortcut_guide_open(!self.shortcut_guide_open, window, cx);
    }

    fn toggle_shortcut_guide_click(
        &mut self,
        _: &gpui::ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_shortcut_guide_open(!self.shortcut_guide_open, window, cx);
    }

    fn set_shortcut_guide_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        if open == self.shortcut_guide_open {
            return;
        }
        if open {
            let return_focus = window.focused(cx);
            if self.command_palette_open {
                self.command_palette_open = false;
                self.command_palette_return_focus = None;
            }
            self.shortcut_guide_return_focus = return_focus;
            self.shortcut_guide_open = true;
            self.shortcut_guide_focus.focus(window);
        } else {
            self.shortcut_guide_open = false;
            if let Some(focus) = self.shortcut_guide_return_focus.take() {
                focus.focus(window);
            } else if self.investigation.is_some() {
                self.investigation_focus.focus(window);
            }
        }
        cx.notify();
    }

    fn palette_commands(&self) -> Vec<PaletteCommandItem> {
        vec![
            PaletteCommandItem {
                command: PaletteCommand::Navigate(Section::Overview),
                label: "Open account audit".into(),
                detail: "Inventory, drift, coverage, and cost signals",
                icon: IconName::Inspector,
                shortcut: None,
            },
            PaletteCommandItem {
                command: PaletteCommand::Navigate(Section::Resources),
                label: "Open resources".into(),
                detail: "Search Cloudflare inventory and health",
                icon: IconName::Frame,
                shortcut: Some("Ctrl/Cmd F"),
            },
            PaletteCommandItem {
                command: PaletteCommand::Navigate(Section::Workers),
                label: "Open Workers".into(),
                detail: "Runtime and observability coverage",
                icon: IconName::SquareTerminal,
                shortcut: None,
            },
            PaletteCommandItem {
                command: PaletteCommand::Navigate(Section::Billing),
                label: "Open cost".into(),
                detail: "Usage-derived Workers projection",
                icon: IconName::ChartPie,
                shortcut: None,
            },
            PaletteCommandItem {
                command: PaletteCommand::Navigate(Section::Connection),
                label: "Open connection".into(),
                detail: "Account, scopes, and collector diagnostics",
                icon: IconName::Globe,
                shortcut: None,
            },
            PaletteCommandItem {
                command: PaletteCommand::Navigate(Section::Settings),
                label: "Open settings".into(),
                detail: "Appearance, motion, and navigation preferences",
                icon: IconName::Settings2,
                shortcut: None,
            },
            PaletteCommandItem {
                command: PaletteCommand::Refresh,
                label: "Sync Cloudflare now".into(),
                detail: "Refresh inventory, usage, and audit signals",
                icon: IconName::Redo2,
                shortcut: Some("Ctrl/Cmd R"),
            },
            PaletteCommandItem {
                command: PaletteCommand::CopyReport,
                label: "Copy audit report".into(),
                detail: "Copy the current Markdown report",
                icon: IconName::Copy,
                shortcut: None,
            },
            PaletteCommandItem {
                command: PaletteCommand::ToggleTheme,
                label: if self.dark {
                    "Switch to light mode".into()
                } else {
                    "Switch to dark mode".into()
                },
                detail: "Change Cedar's native color theme",
                icon: if self.dark {
                    IconName::Sun
                } else {
                    IconName::Moon
                },
                shortcut: None,
            },
        ]
    }

    fn filtered_palette_commands(&self, cx: &App) -> Vec<PaletteCommandItem> {
        let query = self.command_input.read(cx).value().to_lowercase();
        self.palette_commands()
            .into_iter()
            .filter(|item| {
                query.is_empty()
                    || item.label.to_lowercase().contains(&query)
                    || item.detail.to_lowercase().contains(&query)
            })
            .collect()
    }

    fn run_selected_palette_command(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let commands = self.filtered_palette_commands(cx);
        let Some(item) = commands.get(self.palette_selected.min(commands.len().saturating_sub(1)))
        else {
            return;
        };
        self.execute_palette_command(item.command, window, cx);
    }

    fn execute_palette_command(
        &mut self,
        command: PaletteCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_command_palette_open(false, window, cx);
        match command {
            PaletteCommand::Navigate(section) => {
                self.active_section = section;
                self.selected_resource = None;
                self.selected_finding = None;
                self.resource_table
                    .update(cx, |table, cx| table.clear_selection(cx));
            }
            PaletteCommand::Refresh => self.refresh(true, cx),
            PaletteCommand::CopyReport => {
                let findings = build_audit_findings(&self.snapshot, &self.worker_preferences);
                let report = build_audit_report(&self.snapshot, &findings, &self.recent_changes);
                cx.write_to_clipboard(ClipboardItem::new_string(report));
                self.report_copied = true;
            }
            PaletteCommand::ToggleTheme => {
                self.dark = !self.dark;
                Theme::change(
                    if self.dark {
                        ThemeMode::Dark
                    } else {
                        ThemeMode::Light
                    },
                    Some(window),
                    cx,
                );
                apply_component_theme(self.dark, cx);
                if let Err(error) = self
                    .backend
                    .set_preference(THEME_KEY, if self.dark { "dark" } else { "light" })
                {
                    self.error = Some(UiError::new(
                        "Theme preference was not saved",
                        error.to_string(),
                    ));
                }
            }
        }
        cx.notify();
    }

    fn command_palette_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.command_palette_open {
            return;
        }

        let command_count = self.filtered_palette_commands(cx).len();
        match event.keystroke.key.as_str() {
            "down" if command_count > 0 => {
                self.palette_selected = (self.palette_selected + 1).min(command_count - 1);
                self.command_palette_scroll
                    .scroll_to_item(self.palette_selected);
            }
            "up" if command_count > 0 => {
                self.palette_selected = self.palette_selected.saturating_sub(1);
                self.command_palette_scroll
                    .scroll_to_item(self.palette_selected);
            }
            "home" if command_count > 0 => {
                self.palette_selected = 0;
                self.command_palette_scroll.scroll_to_item(0);
            }
            "end" if command_count > 0 => {
                self.palette_selected = command_count - 1;
                self.command_palette_scroll
                    .scroll_to_item(self.palette_selected);
            }
            "tab" => self
                .command_input
                .update(cx, |input, cx| input.focus(window, cx)),
            "enter" if command_count > 0 => self.run_selected_palette_command(window, cx),
            "escape" => self.set_command_palette_open(false, window, cx),
            _ => return,
        }
        window.prevent_default();
        cx.stop_propagation();
        cx.notify();
    }

    fn investigation_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.investigation.is_none()
            || self.command_palette_open
            || self.shortcut_guide_open
            || !self.investigation_focus.is_focused(window)
        {
            return;
        }

        match event.keystroke.key.as_str() {
            "j" => self.set_investigation_cursor(1, cx),
            "k" => self.set_investigation_cursor(-1, cx),
            "enter" => self.open_current_investigation_resource(cx),
            "f" => self.focus_investigation(window, cx),
            "escape" => self.finish_investigation(cx),
            _ => return,
        }
        window.prevent_default();
        cx.stop_propagation();
    }

    fn palette(&self) -> Palette {
        if self.dark {
            Palette::dark()
        } else {
            Palette::light()
        }
    }

    fn render_command_palette(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = self.palette();
        let commands = self.filtered_palette_commands(cx);
        let selected = self.palette_selected.min(commands.len().saturating_sub(1));
        let is_empty = commands.is_empty();
        let results_height = if is_empty {
            180.
        } else {
            (commands.len() as f32 * 58. + 16.).min(460.)
        };

        div()
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .left_0()
            .flex()
            .items_start()
            .justify_center()
            .pt(px(118.))
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .on_key_down(cx.listener(Self::command_palette_key_down))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .bottom_0()
                    .left_0()
                    .bg(color(0x000000).opacity(if self.dark { 0.58 } else { 0.34 }))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.set_command_palette_open(false, window, cx);
                        }),
                    ),
            )
            .child(
                div()
                    .relative()
                    .w(px(620.))
                    .max_h(px(590.))
                    .rounded(px(16.))
                    .border_1()
                    .border_color(palette.border_strong)
                    .bg(palette.raised)
                    .shadow(palette.panel_shadow())
                    .overflow_hidden()
                    .child(
                        div()
                            .p_3()
                            .border_b_1()
                            .border_color(palette.border)
                            .child(Input::new(&self.command_input).cleanable(true)),
                    )
                    .child(
                        div()
                            .id("command-palette-results")
                            .h(px(results_height))
                            .p_2()
                            .overflow_y_scroll()
                            .track_scroll(&self.command_palette_scroll)
                            .when(is_empty, |view| {
                                view.child(
                                    div()
                                        .h(px(180.))
                                        .flex()
                                        .flex_col()
                                        .items_center()
                                        .justify_center()
                                        .text_color(palette.muted)
                                        .child(Icon::new(IconName::Search))
                                        .child(
                                            div()
                                                .pt_3()
                                                .text_size(px(12.))
                                                .child("No matching commands"),
                                        ),
                                )
                            })
                            .when(!is_empty, |view| {
                                view.children(commands.into_iter().enumerate().map(
                                    |(index, item)| {
                                        let command = item.command;
                                        div()
                                            .id(("palette-command", index))
                                            .h(px(58.))
                                            .px_3()
                                            .rounded(px(10.))
                                            .flex()
                                            .items_center()
                                            .gap_3()
                                            .when(index == selected, |row| row.bg(palette.selected))
                                            .hover(|row| row.bg(palette.hover))
                                            .active(|row| row.bg(palette.selected))
                                            .on_hover(cx.listener(move |this, hovered, _, cx| {
                                                if *hovered && this.palette_selected != index {
                                                    this.palette_selected = index;
                                                    cx.notify();
                                                }
                                            }))
                                            .cursor_pointer()
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.execute_palette_command(command, window, cx)
                                            }))
                                            .child(
                                                div()
                                                    .size(px(34.))
                                                    .rounded(px(9.))
                                                    .bg(palette.surface)
                                                    .text_color(palette.foreground)
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .child(Icon::new(item.icon)),
                                            )
                                            .child(
                                                div()
                                                    .min_w_0()
                                                    .flex_grow()
                                                    .child(
                                                        div()
                                                            .text_size(px(13.))
                                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                                            .child(item.label),
                                                    )
                                                    .child(
                                                        div()
                                                            .pt_1()
                                                            .text_size(px(10.))
                                                            .text_color(palette.muted)
                                                            .child(item.detail),
                                                    ),
                                            )
                                            .when_some(item.shortcut, |row, shortcut| {
                                                row.child(shortcut_badge(shortcut, palette))
                                            })
                                    },
                                ))
                            }),
                    )
                    .child(
                        div()
                            .h(px(38.))
                            .px_4()
                            .border_t_1()
                            .border_color(palette.border)
                            .flex()
                            .items_center()
                            .justify_between()
                            .text_size(px(10.))
                            .text_color(palette.subtle)
                            .child("↑↓ navigate  ·  Enter run")
                            .child("Esc close  ·  Ctrl/Cmd K toggle"),
                    ),
            )
            .into_any_element()
    }

    fn shortcut_guide_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.shortcut_guide_open || !self.shortcut_guide_focus.is_focused(window) {
            return;
        }
        if event.keystroke.key == "escape" {
            self.set_shortcut_guide_open(false, window, cx);
        }
        window.prevent_default();
        cx.stop_propagation();
    }

    fn render_shortcut_guide(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let palette = self.palette();
        let sheet_height = (f32::from(window.viewport_size().height) - 48.).min(620.);
        div()
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .left_0()
            .p_6()
            .flex()
            .items_center()
            .justify_center()
            .track_focus(&self.shortcut_guide_focus)
            .on_key_down(cx.listener(Self::shortcut_guide_key_down))
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .bottom_0()
                    .left_0()
                    .bg(color(0x000000).opacity(if self.dark { 0.62 } else { 0.36 }))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.set_shortcut_guide_open(false, window, cx);
                        }),
                    ),
            )
            .child(
                div()
                    .relative()
                    .w(px(760.))
                    .h(px(sheet_height))
                    .flex()
                    .flex_col()
                    .rounded(px(18.))
                    .border_1()
                    .border_color(palette.border_strong)
                    .bg(palette.raised)
                    .shadow(palette.panel_shadow())
                    .overflow_hidden()
                    .child(
                        div()
                            .flex_none()
                            .px_5()
                            .py_4()
                            .border_b_1()
                            .border_color(palette.border)
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_4()
                            .child(
                                div()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .child(
                                        div()
                                            .size(px(38.))
                                            .rounded(px(11.))
                                            .bg(palette.accent_soft)
                                            .text_color(palette.accent)
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(Icon::new(IconName::BookOpen)),
                                    )
                                    .child(
                                        div()
                                            .min_w_0()
                                            .child(
                                                div()
                                                    .text_size(px(16.))
                                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                                    .child("Keyboard shortcuts"),
                                            )
                                            .child(
                                                div()
                                                    .pt_1()
                                                    .text_size(px(11.))
                                                    .text_color(palette.muted)
                                                    .child("Move through Cedar without leaving the keyboard."),
                                            ),
                                    ),
                            )
                            .child(
                                Button::new("close-shortcut-guide")
                                    .ghost()
                                    .icon(IconName::Close)
                                    .tooltip("Close · Esc")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.set_shortcut_guide_open(false, window, cx);
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .min_h_0()
                            .flex_grow()
                            .p_5()
                            .flex()
                            .items_start()
                            .gap_3()
                            .overflow_y_scrollbar()
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .gap_3()
                                    .child(shortcut_group(
                                        IconName::LayoutDashboard,
                                        "Workspace",
                                        &[
                                            ("Command palette", "Ctrl/Cmd K"),
                                            ("Resource search", "Ctrl/Cmd F"),
                                            ("Toggle sidebar", "Ctrl/Cmd B"),
                                            ("Sync dashboard", "Ctrl/Cmd R"),
                                        ],
                                        palette,
                                    ))
                                    .child(shortcut_group(
                                        IconName::Inspector,
                                        "Investigation",
                                        &[
                                            ("Next evidence", "J"),
                                            ("Previous evidence", "K"),
                                            ("Inspect resource", "Enter"),
                                            ("Focus context", "F"),
                                        ],
                                        palette,
                                    )),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .gap_3()
                                    .child(shortcut_group(
                                        IconName::PanelRight,
                                        "Inspector",
                                        &[
                                            ("Previous resource", "Alt ←"),
                                            ("Next resource", "Alt →"),
                                            ("Close inspector", "Esc"),
                                        ],
                                        palette,
                                    ))
                                    .child(shortcut_group(
                                        IconName::Asterisk,
                                        "General",
                                        &[
                                            ("Shortcut guide", "?"),
                                            ("Navigate menus", "↑ / ↓"),
                                            ("Activate", "Enter"),
                                            ("Dismiss", "Esc"),
                                        ],
                                        palette,
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .h(px(40.))
                            .px_5()
                            .border_t_1()
                            .border_color(palette.border)
                            .flex()
                            .items_center()
                            .justify_between()
                            .text_size(px(9.))
                            .text_color(palette.subtle)
                            .child("CEDAR KEYBOARD REFERENCE")
                            .child("Press ? anytime outside a text field"),
                    ),
            )
            .into_any_element()
    }

    fn render_titlebar(&self, window: &Window) -> AnyElement {
        let palette = self.palette();
        if cfg!(target_os = "macos") {
            return TitleBar::new()
                .bg(palette.sidebar)
                .border_color(palette.border)
                .child(
                    div()
                        .h_full()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(cedar_mark(20., palette, self.dark))
                        .child(
                            div()
                                .text_size(px(11.))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child("Cedar"),
                        ),
                )
                .into_any_element();
        }

        let maximized = window.is_maximized();
        div()
            .id("cedar-title-bar")
            .h(px(34.))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .border_b_1()
            .border_color(palette.border)
            .bg(palette.sidebar)
            .child(
                div()
                    .h_full()
                    .flex_grow()
                    .flex()
                    .items_center()
                    .gap_2()
                    .pl_3()
                    .window_control_area(WindowControlArea::Drag)
                    .on_mouse_down(gpui::MouseButton::Left, |event, window, _| {
                        if event.click_count > 1 {
                            window.zoom_window();
                        }
                    })
                    .child(cedar_mark(20., palette, self.dark))
                    .child(
                        div()
                            .text_size(px(11.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("Cedar"),
                    ),
            )
            .child(
                div()
                    .h_full()
                    .flex()
                    .items_center()
                    .child(
                        Button::new("window-minimize")
                            .ghost()
                            .compact()
                            .icon(IconName::WindowMinimize)
                            .tooltip("Minimize")
                            .w(px(40.))
                            .h_full()
                            .rounded_none()
                            .on_click(|_, window, cx| {
                                cx.stop_propagation();
                                window.minimize_window();
                            }),
                    )
                    .child(
                        Button::new("window-maximize")
                            .ghost()
                            .compact()
                            .icon(if maximized {
                                IconName::WindowRestore
                            } else {
                                IconName::WindowMaximize
                            })
                            .tooltip(if maximized { "Restore" } else { "Maximize" })
                            .w(px(40.))
                            .h_full()
                            .rounded_none()
                            .on_click(|_, window, cx| {
                                cx.stop_propagation();
                                window.zoom_window();
                            }),
                    )
                    .child(
                        Button::new("window-close")
                            .ghost()
                            .compact()
                            .icon(IconName::WindowClose)
                            .tooltip("Close")
                            .w(px(40.))
                            .h_full()
                            .rounded_none()
                            .hover(move |style| style.bg(palette.bad).text_color(color(0xffffff)))
                            .on_click(|_, window, cx| {
                                cx.stop_propagation();
                                window.remove_window();
                            }),
                    ),
            )
            .into_any_element()
    }

    fn render_collapsed_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = self.palette();
        let items = [
            (Section::Overview, "Audit", "01", IconName::Inspector),
            (Section::Resources, "Resources", "02", IconName::Frame),
            (Section::Workers, "Workers", "03", IconName::SquareTerminal),
            (Section::Billing, "Cost", "04", IconName::ChartPie),
            (Section::Connection, "Connection", "05", IconName::Globe),
            (Section::Settings, "Settings", "06", IconName::Settings2),
        ];
        div()
            .w(px(56.))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .py_2()
            .bg(palette.sidebar)
            .border_r_1()
            .border_color(palette.border)
            .child(
                div()
                    .h(px(32.))
                    .flex()
                    .items_center()
                    .child(cedar_mark(24., palette, self.dark)),
            )
            .child(
                Button::new("expand-sidebar")
                    .ghost()
                    .icon(IconName::PanelLeftOpen)
                    .tooltip("Expand sidebar · Ctrl/Cmd B")
                    .w(px(36.))
                    .h(px(36.))
                    .on_click(cx.listener(Self::toggle_sidebar_click)),
            )
            .child(div().w(px(28.)).border_t_1().border_color(palette.border))
            .children(items.into_iter().map(|(section, label, _, icon)| {
                let selected = self.active_section == section;
                Button::new(SharedString::from(format!("compact-nav-{label}")))
                    .ghost()
                    .icon(icon)
                    .tooltip(label)
                    .w(px(36.))
                    .h(px(36.))
                    .when(selected, |button| {
                        button.bg(palette.selected).text_color(palette.accent)
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.active_section = section;
                        this.selected_resource = None;
                        this.selected_finding = None;
                        this.resource_table
                            .update(cx, |table, cx| table.clear_selection(cx));
                        cx.notify();
                    }))
            }))
            .child(div().flex_grow())
            .into_any_element()
    }

    fn render_resource_explorer(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let palette = self.palette();
        let workers_only = self.active_section == Section::Workers;
        let investigation_keys = self.investigation.as_ref().and_then(|investigation| {
            (!investigation.resource_keys.is_empty()).then_some(&investigation.resource_keys)
        });
        let mut groups = BTreeMap::<String, Vec<ResourceRow>>::new();
        for resource in self
            .snapshot
            .resources
            .iter()
            .filter(|resource| !workers_only || resource.kind == "worker")
            .filter(|resource| {
                investigation_keys
                    .is_none_or(|keys| keys.iter().any(|key| key == &resource_key(resource)))
            })
            .cloned()
        {
            groups
                .entry(resource.kind.clone())
                .or_default()
                .push(resource);
        }
        for resources in groups.values_mut() {
            resources.sort_by(|left, right| left.name.cmp(&right.name));
        }
        let resource_count = groups.values().map(Vec::len).sum::<usize>();
        let mut fingerprint = workers_only.to_string();
        for resources in groups.values() {
            for resource in resources {
                fingerprint.push('\u{1f}');
                fingerprint.push_str(&resource_key(resource));
                fingerprint.push('\u{1f}');
                fingerprint.push_str(&resource.name);
            }
        }
        if self.resource_tree_fingerprint != fingerprint {
            let kind_items = groups
                .iter()
                .map(|(kind, resources)| {
                    TreeItem::new(
                        format!("kind:{kind}"),
                        format!("{} · {}", resource_kind_label(kind), resources.len()),
                    )
                    .expanded(true)
                    .children(resources.iter().map(|resource| {
                        TreeItem::new(
                            format!("resource:{}", resource_key(resource)),
                            resource.name.clone(),
                        )
                    }))
                })
                .collect::<Vec<_>>();
            self.resource_tree
                .update(cx, |tree, cx| tree.set_items(kind_items, cx));
            self.resource_tree_fingerprint = fingerprint;
        }

        let owner = cx.entity().downgrade();
        let selected_resource = self.selected_resource.clone();
        div()
            .min_h_0()
            .flex_grow()
            .flex()
            .flex_col()
            .pt_4()
            .child(
                div()
                    .px_4()
                    .pb_2()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(px(10.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(palette.subtle)
                    .child("Explorer")
                    .child(resource_count.to_string()),
            )
            .child(div().min_h_0().flex_grow().px_2().child(tree(
                &self.resource_tree,
                move |ix, entry, _, _, cx| {
                    let item = entry.item();
                    let id = item.id.to_string();
                    let selected_key = id.strip_prefix("resource:").map(str::to_string);
                    let resource_meta = selected_key.as_ref().and_then(|key| {
                        owner.upgrade().and_then(|owner| {
                            owner
                                .read(cx)
                                .snapshot
                                .resources
                                .iter()
                                .find(|resource| resource_key(resource) == *key)
                                .map(|resource| (resource.kind.clone(), resource.status.clone()))
                        })
                    });
                    let selected = selected_key
                        .as_ref()
                        .is_some_and(|key| selected_resource.as_ref() == Some(key));
                    let icon = if item.is_folder() {
                        if item.is_expanded() {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        }
                    } else {
                        resource_meta
                            .as_ref()
                            .map(|(kind, _)| resource_kind_icon(kind))
                            .unwrap_or(IconName::Frame)
                    };
                    let owner = owner.clone();

                    ListItem::new(ix)
                        .h(px(31.))
                        .rounded(px(7.))
                        .text_size(px(11.))
                        .when(selected, |row| row.bg(palette.selected))
                        .when_some(selected_key, |row, key| {
                            row.on_click(move |_, _, cx| {
                                if let Some(owner) = owner.upgrade() {
                                    owner.update(cx, |this, cx| {
                                        this.open_resource(key.clone(), true, cx);
                                        this.sync_resource_table_selection(cx);
                                    });
                                }
                            })
                        })
                        .child(
                            div()
                                .pl(px(4. + entry.depth() as f32 * 12.))
                                .min_w_0()
                                .w_full()
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_color(if selected {
                                    palette.foreground
                                } else {
                                    palette.muted
                                })
                                .child(Icon::new(icon).text_color(if selected {
                                    palette.accent
                                } else {
                                    palette.subtle
                                }))
                                .child(
                                    div()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .font_weight(if selected {
                                            gpui::FontWeight::SEMIBOLD
                                        } else {
                                            gpui::FontWeight::MEDIUM
                                        })
                                        .child(item.label.clone()),
                                ),
                        )
                },
            )))
            .into_any_element()
    }

    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.sidebar_collapsed {
            return self.render_collapsed_sidebar(cx);
        }
        let palette = self.palette();
        let items = [
            (Section::Overview, "Audit", "01", IconName::Inspector),
            (Section::Resources, "Resources", "02", IconName::Frame),
            (Section::Workers, "Workers", "03", IconName::SquareTerminal),
            (Section::Billing, "Cost", "04", IconName::ChartPie),
            (Section::Connection, "Connection", "05", IconName::Globe),
            (Section::Settings, "Settings", "06", IconName::Settings2),
        ];
        let account_name = self
            .connection
            .as_ref()
            .and_then(|connection| connection.account.as_ref())
            .map(|account| account.name.clone())
            .unwrap_or_else(|| "No account connected".into());
        let connected = self.connected();
        let show_resource_explorer = connected
            && matches!(self.active_section, Section::Resources | Section::Workers)
            && !self.snapshot.resources.is_empty();
        let resource_explorer = show_resource_explorer.then(|| self.render_resource_explorer(cx));

        div()
            .w(px(216.))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(palette.sidebar)
            .border_r_1()
            .border_color(palette.border)
            .child(
                div()
                    .px_4()
                    .pt_4()
                    .pb_3()
                    .flex()
                    .items_start()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(cedar_mark(28., palette, self.dark))
                            .child(
                                div()
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .child("Cedar"),
                                    )
                                    .child(
                                        div()
                                            .pt_1()
                                            .text_size(px(9.))
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .text_color(palette.subtle)
                                            .child("OPERATIONS"),
                                    ),
                            ),
                    )
                    .child(
                        Button::new("collapse-sidebar")
                            .ghost()
                            .icon(IconName::PanelLeftClose)
                            .tooltip("Collapse sidebar · Ctrl/Cmd B")
                            .w(px(30.))
                            .h(px(30.))
                            .on_click(cx.listener(Self::toggle_sidebar_click)),
                    ),
            )
            .child(
                div()
                    .px_2()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(items.into_iter().map(|(section, label, code, icon)| {
                        let selected = self.active_section == section;
                        Button::new(SharedString::from(format!("nav-{label}")))
                            .ghost()
                            .w_full()
                            .h(px(40.))
                            .px_0()
                            .rounded(px(8.))
                            .text_size(px(12.))
                            .text_color(if selected {
                                palette.foreground
                            } else {
                                palette.muted
                            })
                            .when(selected, |row| row.bg(palette.selected))
                            .hover(|row| row.bg(palette.hover).text_color(palette.foreground))
                            .tooltip(format!("Open {label}"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.active_section = section;
                                this.selected_resource = None;
                                this.selected_finding = None;
                                this.resource_table
                                    .update(cx, |table, cx| table.clear_selection(cx));
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .relative()
                                    .w(px(184.))
                                    .h_full()
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .when(selected, |row| {
                                        row.child(
                                            div()
                                                .absolute()
                                                .left_0()
                                                .top(px(9.))
                                                .bottom(px(9.))
                                                .w(px(2.))
                                                .rounded_full()
                                                .bg(palette.accent),
                                        )
                                    })
                                    .child(
                                        div()
                                            .size(px(24.))
                                            .flex_none()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .text_color(if selected {
                                                palette.accent
                                            } else {
                                                palette.muted
                                            })
                                            .child(Icon::new(icon)),
                                    )
                                    .child(
                                        div()
                                            .font_weight(if selected {
                                                gpui::FontWeight::SEMIBOLD
                                            } else {
                                                gpui::FontWeight::MEDIUM
                                            })
                                            .child(label),
                                    )
                                    .child(
                                        div()
                                            .ml_auto()
                                            .font_family(FONT_MONO)
                                            .text_size(px(9.))
                                            .text_color(if selected {
                                                palette.accent
                                            } else {
                                                palette.subtle
                                            })
                                            .child(code),
                                    ),
                            )
                    })),
            )
            .when_some(resource_explorer, |sidebar, explorer| {
                sidebar.child(explorer)
            })
            .when(!show_resource_explorer, |sidebar| {
                sidebar.child(div().flex_grow())
            })
            .child(
                Button::new("connection-card")
                    .ghost()
                    .mx_2()
                    .mb_2()
                    .h(px(64.))
                    .px_3()
                    .rounded(px(8.))
                    .bg(palette.surface)
                    .hover(|card| card.bg(palette.hover))
                    .tooltip("Open connection settings")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.active_section = Section::Connection;
                        this.selected_resource = None;
                        this.selected_finding = None;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w(px(176.))
                            .flex()
                            .flex_col()
                            .items_start()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(div().size(px(8.)).rounded_full().bg(if connected {
                                        palette.good
                                    } else {
                                        palette.warn
                                    }))
                                    .child(
                                        div()
                                            .text_size(px(10.))
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(if connected {
                                                palette.good
                                            } else {
                                                palette.warn
                                            })
                                            .child(if connected {
                                                "Connected"
                                            } else {
                                                "Setup required"
                                            }),
                                    ),
                            )
                            .child(
                                div()
                                    .pt_2()
                                    .text_size(px(11.))
                                    .text_color(palette.muted)
                                    .overflow_hidden()
                                    .child(account_name),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_topbar(&self, compact: bool, cx: &mut Context<Self>) -> AnyElement {
        let palette = self.palette();
        let sync_status: SharedString = if self.syncing {
            "SYNC IN PROGRESS".into()
        } else if self.snapshot.generated_at.is_empty() {
            "NOT YET SYNCED".into()
        } else {
            format_updated_at(&self.snapshot.generated_at).into()
        };
        let overflow_owner = cx.entity().downgrade();
        let overflow_status = sync_status.clone();
        let overflow_syncing = self.syncing;
        let overflow_connected = self.connected();
        let (section_code, title, description) = match self.active_section {
            Section::Overview => (
                "01",
                "Account audit",
                "Inventory, drift, coverage, and cost signals",
            ),
            Section::Resources => ("02", "Resources", "Cloudflare inventory and health"),
            Section::Workers => ("03", "Workers", "Runtime and observability coverage"),
            Section::Billing => ("04", "Cost", "Usage-derived Workers Paid projection"),
            Section::Connection => (
                "05",
                "Connection",
                "Token scope, account, and collector diagnostics",
            ),
            Section::Settings => (
                "06",
                "Settings",
                "Appearance, motion, and navigation preferences",
            ),
        };
        div()
            .h(px(if compact { 66. } else { 76. }))
            .flex_none()
            .when(compact, |bar| bar.px_4())
            .when(!compact, |bar| bar.px_6())
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .bg(palette.background)
            .border_b_1()
            .border_color(palette.border.opacity(0.72))
            .child(
                div()
                    .min_w_0()
                    .flex_grow()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(if compact { 18. } else { 21. }))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .font_family(FONT_MONO)
                                    .text_size(px(9.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(palette.accent)
                                    .child(section_code),
                            )
                            .child(
                                div()
                                    .h(px(10.))
                                    .border_l_1()
                                    .border_color(palette.border_strong),
                            )
                            .when(!compact, |meta| {
                                meta.child(
                                    div()
                                        .min_w_0()
                                        .text_size(px(11.))
                                        .text_color(palette.muted)
                                        .text_ellipsis()
                                        .child(description),
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(if compact { px(4.) } else { px(8.) })
                    .when(!compact, |actions| {
                        actions
                            .child(
                                Button::new("commands")
                                    .ghost()
                                    .icon(IconName::Search)
                                    .label("Commands")
                                    .tooltip("Command palette · Ctrl/Cmd K")
                                    .on_click(cx.listener(Self::toggle_command_palette_click)),
                            )
                            .child(
                                Button::new("shortcut-guide")
                                    .ghost()
                                    .icon(IconName::BookOpen)
                                    .tooltip("Keyboard shortcuts · ?")
                                    .on_click(cx.listener(Self::toggle_shortcut_guide_click)),
                            )
                    })
                    .child(
                        div()
                            .p_1()
                            .rounded(px(8.))
                            .border_1()
                            .border_color(palette.border)
                            .bg(palette.panel)
                            .flex()
                            .items_center()
                            .gap_1()
                            .children(["24h", "7d", "30d"].into_iter().map(|range| {
                                Button::new(SharedString::from(format!("range-{range}")))
                                    .label(range)
                                    .when(self.range == range, |button| {
                                        button.bg(palette.selected).text_color(palette.accent)
                                    })
                                    .when(self.range != range, |button| button.ghost())
                                    .disabled(self.syncing || !self.connected())
                                    .on_click(
                                        cx.listener(move |this, _, _, cx| {
                                            this.set_range(range, cx)
                                        }),
                                    )
                            })),
                    )
                    .when(!compact && self.connected(), |actions| {
                        actions.child(
                            div()
                                .max_w(px(190.))
                                .h(px(32.))
                                .mx_1()
                                .px_3()
                                .rounded(px(8.))
                                .border_1()
                                .border_color(palette.border)
                                .bg(palette.panel)
                                .flex()
                                .items_center()
                                .gap_2()
                                .font_family(FONT_MONO)
                                .text_size(px(10.))
                                .text_color(if self.syncing {
                                    palette.accent
                                } else {
                                    palette.subtle
                                })
                                .child(status_dot(if self.syncing {
                                    Tone::Neutral
                                } else {
                                    Tone::Good
                                }))
                                .child(div().text_ellipsis().child(sync_status.clone())),
                        )
                    })
                    .when(!compact, |actions| {
                        actions.child(
                            Button::new("refresh")
                                .primary()
                                .icon(IconName::Redo2)
                                .label(if self.syncing { "Syncing…" } else { "Sync" })
                                .tooltip(if self.syncing {
                                    "Syncing Cloudflare"
                                } else {
                                    "Sync Cloudflare · Ctrl/Cmd R"
                                })
                                .loading(self.syncing && !self.reduced_motion)
                                .disabled(self.syncing || !self.connected())
                                .on_click(cx.listener(|this, _, _, cx| this.refresh(true, cx))),
                        )
                    })
                    .when(compact, |actions| {
                        actions.child(
                            Button::new("topbar-overflow")
                                .ghost()
                                .icon(IconName::Ellipsis)
                                .tooltip("More actions")
                                .dropdown_menu_with_anchor(Corner::TopRight, move |menu, _, _| {
                                    let commands_owner = overflow_owner.clone();
                                    let sync_owner = overflow_owner.clone();
                                    menu.item(PopupMenuItem::label(overflow_status.clone()))
                                        .item(
                                            PopupMenuItem::new("Commands")
                                                .icon(IconName::Search)
                                                .on_click(move |_, window, cx| {
                                                    let Some(owner) = commands_owner.upgrade()
                                                    else {
                                                        return;
                                                    };
                                                    owner.update(cx, |this, cx| {
                                                        this.set_command_palette_open(
                                                            true, window, cx,
                                                        );
                                                    });
                                                }),
                                        )
                                        .item(
                                            PopupMenuItem::new(if overflow_syncing {
                                                "Syncing Cloudflare"
                                            } else {
                                                "Sync Cloudflare now"
                                            })
                                            .icon(IconName::Redo2)
                                            .disabled(!overflow_connected || overflow_syncing)
                                            .on_click(move |_, _, cx| {
                                                let Some(owner) = sync_owner.upgrade() else {
                                                    return;
                                                };
                                                owner.update(cx, |this, cx| {
                                                    this.refresh(true, cx);
                                                });
                                            }),
                                        )
                                }),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_investigation_ribbon(
        &self,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let investigation = self.investigation.as_ref()?;
        let palette = self.palette();
        let current_name = self
            .current_investigation_resource()
            .map(|resource| resource.name.clone());
        let resource_count = investigation.resource_keys.len();
        let position = if resource_count == 0 {
            "Account-level finding".into()
        } else {
            format!("{} of {resource_count}", investigation.cursor + 1)
        };
        let can_navigate = resource_count > 1;
        let can_inspect = resource_count > 0;

        Some(
            div()
                .h(px(58.))
                .flex_none()
                .when(compact, |ribbon| ribbon.px_4())
                .when(!compact, |ribbon| ribbon.px_6())
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .border_b_1()
                .border_color(palette.accent.opacity(0.34))
                .bg(palette.raised)
                .child(
                    div()
                        .min_w_0()
                        .flex_grow()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(status_dot(investigation.finding.tone))
                        .child(
                            div()
                                .min_w_0()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            div()
                                                .text_size(px(11.))
                                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                                .text_ellipsis()
                                                .child(investigation.finding.title.clone()),
                                        )
                                        .child(
                                            div()
                                                .font_family(FONT_MONO)
                                                .text_size(px(10.))
                                                .text_color(palette.accent)
                                                .child(position),
                                        ),
                                )
                                .child(
                                    div()
                                        .pt_1()
                                        .text_size(px(9.))
                                        .text_color(palette.muted)
                                        .text_ellipsis()
                                        .child(current_name.unwrap_or_else(|| {
                                            "No resource-specific evidence · F opens the target section"
                                                .into()
                                        })),
                                ),
                        ),
                )
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap_1()
                        .when(!compact, |actions| {
                            actions.child(
                                div()
                                    .pr_2()
                                    .font_family(FONT_MONO)
                                    .text_size(px(10.))
                                    .text_color(palette.subtle)
                                    .child("J/K NAVIGATE · ENTER INSPECT · F FOCUS"),
                            )
                        })
                        .child(
                            Button::new("investigation-return-audit")
                                .ghost()
                                .icon(IconName::LayoutDashboard)
                                .when(!compact, |button| button.label("Audit"))
                                .tooltip("Return to the audit while keeping this investigation")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.active_section = Section::Overview;
                                    this.selected_resource = None;
                                    this.selected_finding = None;
                                    this.resource_table
                                        .update(cx, |table, cx| table.clear_selection(cx));
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("investigation-previous")
                                .ghost()
                                .icon(IconName::ArrowLeft)
                                .tooltip("Previous evidence · K")
                                .disabled(!can_navigate)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.set_investigation_cursor(-1, cx)
                                })),
                        )
                        .child(
                            Button::new("investigation-next")
                                .ghost()
                                .icon(IconName::ArrowRight)
                                .tooltip("Next evidence · J")
                                .disabled(!can_navigate)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.set_investigation_cursor(1, cx)
                                })),
                        )
                        .child(
                            Button::new("investigation-inspect")
                                .ghost()
                                .icon(IconName::Eye)
                                .tooltip("Inspect selected resource · Enter")
                                .disabled(!can_inspect)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.open_current_investigation_resource(cx)
                                })),
                        )
                        .child(
                            Button::new("investigation-focus")
                                .ghost()
                                .icon(IconName::Search)
                                .tooltip("Focus investigation · F")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.focus_investigation(window, cx)
                                })),
                        )
                        .child(
                            Button::new("investigation-close")
                                .ghost()
                                .icon(IconName::Close)
                                .tooltip("End investigation · Esc")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.finish_investigation(cx)
                                })),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_loading_state(&self) -> AnyElement {
        let palette = self.palette();
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .px_4()
                    .py_3()
                    .rounded(px(10.))
                    .border_1()
                    .border_color(palette.border)
                    .bg(palette.surface)
                    .flex()
                    .items_center()
                    .gap_3()
                    .text_color(palette.muted)
                    .child(
                        div()
                            .size(px(28.))
                            .rounded(px(8.))
                            .bg(palette.accent_soft)
                            .text_color(palette.accent)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Icon::new(IconName::LoaderCircle)),
                    )
                    .child(
                        div()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(palette.foreground)
                                    .child("Building account snapshot"),
                            )
                            .child(
                                div()
                                    .pt_1()
                                    .text_size(px(10.))
                                    .child("Collecting inventory, usage, and audit signals"),
                            ),
                    ),
            )
            .child(
                div()
                    .rounded(px(11.))
                    .border_1()
                    .border_color(palette.border)
                    .bg(palette.panel)
                    .p_5()
                    .flex()
                    .gap_5()
                    .children((0..4).map(|_| {
                        div()
                            .min_w_0()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(loading_block(
                                Some(px(82.)),
                                px(10.),
                                false,
                                self.reduced_motion,
                                palette,
                            ))
                            .child(loading_block(
                                Some(px(126.)),
                                px(26.),
                                false,
                                self.reduced_motion,
                                palette,
                            ))
                            .child(loading_block(
                                Some(px(96.)),
                                px(9.),
                                true,
                                self.reduced_motion,
                                palette,
                            ))
                    })),
            )
            .child(
                panel(palette)
                    .child(
                        div()
                            .pb_5()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(loading_block(
                                Some(px(170.)),
                                px(18.),
                                false,
                                self.reduced_motion,
                                palette,
                            ))
                            .child(loading_block(
                                Some(px(290.)),
                                px(10.),
                                true,
                                self.reduced_motion,
                                palette,
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_3()
                            .child(loading_block(
                                Some(px(470.)),
                                px(172.),
                                false,
                                self.reduced_motion,
                                palette,
                            ))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_grow()
                                    .flex()
                                    .flex_col()
                                    .gap_3()
                                    .child(loading_block(
                                        None,
                                        px(78.),
                                        false,
                                        self.reduced_motion,
                                        palette,
                                    ))
                                    .child(loading_block(
                                        None,
                                        px(78.),
                                        true,
                                        self.reduced_motion,
                                        palette,
                                    )),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_overview(&self, compact: bool, cx: &mut Context<Self>) -> AnyElement {
        let palette = self.palette();
        let findings = build_audit_findings(&self.snapshot, &self.worker_preferences);
        let critical_findings = findings
            .iter()
            .filter(|finding| finding.tone == Tone::Bad)
            .count();
        let review_findings = findings
            .iter()
            .filter(|finding| finding.tone == Tone::Warn)
            .count();
        let actionable_findings = critical_findings + review_findings;
        let finding_count = findings.len();
        let healthy = self
            .snapshot
            .health
            .iter()
            .filter(|health| health.status == "ok")
            .count();
        let primary_finding = findings.first().cloned();
        let investigation_finding = primary_finding.clone();
        let secondary_findings = findings.into_iter().skip(1).collect::<Vec<_>>();
        let overview_tone = if critical_findings > 0 {
            Tone::Bad
        } else if review_findings > 0 {
            Tone::Warn
        } else {
            Tone::Good
        };
        let overview_icon = match overview_tone {
            Tone::Bad => IconName::TriangleAlert,
            Tone::Warn => IconName::Info,
            _ => IconName::CircleCheck,
        };
        let overview_title = if actionable_findings == 0 {
            "Account is operating normally".to_string()
        } else if actionable_findings == 1 {
            "1 item needs attention".to_string()
        } else {
            format!("{actionable_findings} items need attention")
        };
        let overview_detail = if critical_findings > 0 {
            format!(
                "{critical_findings} critical · {review_findings} to review · latest Cloudflare snapshot"
            )
        } else if review_findings > 0 {
            format!("No critical failures · {review_findings} items to review")
        } else {
            "No critical or review-level audit findings in the latest snapshot".into()
        };
        let generated_at = if self.snapshot.generated_at.is_empty() {
            "Not synced".into()
        } else {
            self.snapshot.generated_at.clone()
        };
        let activity_time = format_updated_at(&self.snapshot.generated_at)
            .trim_start_matches("UPDATED ")
            .to_string();
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                panel(palette)
                    .child(
                        div()
                            .flex()
                            .when(compact, |summary| summary.flex_col())
                            .when(!compact, |summary| summary.items_start().justify_between())
                            .gap_4()
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_grow()
                                    .when(compact, |health| health.w_full())
                                    .flex()
                                    .items_start()
                                    .gap_4()
                                    .child(
                                        div()
                                            .size(px(50.))
                                            .flex_none()
                                            .rounded(px(14.))
                                            .bg(tone_color(overview_tone, palette).opacity(0.12))
                                            .text_color(tone_color(overview_tone, palette))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(Icon::new(overview_icon)),
                                    )
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex_grow()
                                            .child(
                                                div()
                                                    .text_size(px(10.))
                                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                                    .text_color(tone_color(overview_tone, palette))
                                                    .child("ACCOUNT HEALTH"),
                                            )
                                            .child(
                                                div()
                                                    .pt_2()
                                                    .text_size(px(22.))
                                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                                    .child(overview_title),
                                            )
                                            .child(
                                                div()
                                                    .pt_2()
                                                    .text_size(px(11.))
                                                    .text_color(palette.muted)
                                                    .child(overview_detail),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .flex()
                                    .flex_wrap()
                                    .gap_2()
                                    .when_some(investigation_finding, |actions, finding| {
                                        actions.child(
                                            Button::new("investigate-top-finding")
                                                .primary()
                                                .icon(IconName::Inspector)
                                                .label("Investigate top issue")
                                                .tooltip("Open the highest-priority finding")
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        this.start_investigation(
                                                            finding.clone(),
                                                            window,
                                                            cx,
                                                        )
                                                    },
                                                )),
                                        )
                                    })
                                    .child(
                                        Button::new("copy-overview-report")
                                            .ghost()
                                            .icon(if self.report_copied {
                                                IconName::Check
                                            } else {
                                                IconName::Copy
                                            })
                                            .label(if self.report_copied {
                                                "Copied"
                                            } else {
                                                "Copy report"
                                            })
                                            .tooltip("Copy the audit report as Markdown")
                                            .on_click(cx.listener(Self::copy_report)),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .pt_5()
                            .flex()
                            .flex_wrap()
                            .gap_4()
                            .child(overview_signal(
                                IconName::Frame,
                                "Resources",
                                self.snapshot.resources.len().to_string(),
                                "Discovered inventory",
                                palette,
                            ))
                            .child(overview_signal(
                                IconName::ChartPie,
                                "Worker requests",
                                compact_number(self.snapshot.metrics.worker_requests),
                                self.range,
                                palette,
                            ))
                            .child(overview_signal(
                                IconName::Calendar,
                                "Projected cost",
                                self.snapshot
                                    .metrics
                                    .cost_usd
                                    .map(money)
                                    .unwrap_or_else(|| "N/A".into()),
                                "Workers Paid",
                                palette,
                            ))
                            .child(overview_signal(
                                IconName::CircleCheck,
                                "Collector health",
                                format!("{healthy}/{}", self.snapshot.health.len()),
                                "Surfaces healthy",
                                palette,
                            )),
                    ),
            )
            .child(
                div()
                    .flex()
                    .when(compact, |workspace| workspace.flex_col())
                    .when(!compact, |workspace| workspace.items_start())
                    .gap_4()
                    .child(
                        div()
                            .min_w_0()
                            .flex_grow()
                            .flex()
                            .flex_col()
                            .gap_4()
                            .child(
                                panel(palette)
                                    .child(
                                        div()
                                            .flex()
                                            .justify_between()
                                            .items_center()
                                            .child(section_heading(
                                                IconName::TriangleAlert,
                                                "Priority queue",
                                                "Ranked audit findings and next actions",
                                                palette,
                                            ))
                                            .child(
                                                div()
                                                    .px_2()
                                                    .py_1()
                                                    .rounded(px(6.))
                                                    .bg(palette.surface)
                                                    .font_family(FONT_MONO)
                                                    .text_size(px(9.))
                                                    .text_color(palette.muted)
                                                    .child(format!("{finding_count} FINDINGS")),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_2()
                                            .when_some(primary_finding, |view, finding| {
                                                view.child(finding_card(
                                                    finding, 1, true, palette, cx,
                                                ))
                                            })
                                            .children(
                                                secondary_findings.into_iter().enumerate().map(
                                                    |(index, finding)| {
                                                        finding_card(
                                                            finding,
                                                            index + 2,
                                                            false,
                                                            palette,
                                                            cx,
                                                        )
                                                    },
                                                ),
                                            ),
                                    ),
                            )
                            .child(
                                panel(palette)
                                    .child(section_heading(
                                        IconName::ChartPie,
                                        "Usage signals",
                                        "Current Cloudflare analytics window",
                                        palette,
                                    ))
                                    .child(div().flex().flex_wrap().gap_2().children(
                                        self.snapshot.usage_panels.iter().enumerate().map(
                                            |(index, usage)| {
                                                let trend = usage_trend_summary(&usage.points);
                                                let sparkline_id = SharedString::from(format!(
                                                    "usage-sparkline-{}-{}",
                                                    usage.id, self.snapshot.generated_at
                                                ));
                                                div()
                                                    .id(SharedString::from(format!(
                                                        "usage-signal-{}",
                                                        usage.id
                                                    )))
                                                    .relative()
                                                    .min_w(px(150.))
                                                    .flex_1()
                                                    .p_4()
                                                    .rounded(px(8.))
                                                    .border_1()
                                                    .border_color(palette.border.opacity(0.0))
                                                    .bg(palette.surface)
                                                    .hover(|card| {
                                                        card.border_color(palette.border_strong)
                                                            .bg(palette.hover)
                                                    })
                                                    .child(
                                                        div()
                                                            .flex()
                                                            .items_center()
                                                            .justify_between()
                                                            .gap_2()
                                                            .child(
                                                                div()
                                                                    .min_w_0()
                                                                    .text_size(px(10.))
                                                                    .text_color(palette.muted)
                                                                    .child(usage.title.clone()),
                                                            )
                                                            .child(status_dot(
                                                                match usage.tone.as_str() {
                                                                    "bad" => Tone::Bad,
                                                                    "warn" => Tone::Warn,
                                                                    "good" => Tone::Good,
                                                                    _ => Tone::Neutral,
                                                                },
                                                            )),
                                                    )
                                                    .child(
                                                        div()
                                                            .pt_3()
                                                            .font_family(FONT_MONO)
                                                            .text_size(px(20.))
                                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                                            .child(usage.value.clone()),
                                                    )
                                                    .child(usage_sparkline(
                                                        &usage.points,
                                                        sparkline_id,
                                                        palette,
                                                        self.reduced_motion,
                                                    ))
                                                    .child(
                                                        div()
                                                            .pt_2()
                                                            .flex()
                                                            .items_center()
                                                            .justify_between()
                                                            .gap_2()
                                                            .text_size(px(10.))
                                                            .text_color(palette.subtle)
                                                            .child(usage.detail.clone())
                                                            .when_some(trend, |row, trend| {
                                                                row.child(
                                                                    div()
                                                                        .flex_none()
                                                                        .font_family(FONT_MONO)
                                                                        .text_color(palette.muted)
                                                                        .child(trend),
                                                                )
                                                            }),
                                                    )
                                                    .child(
                                                        div()
                                                            .absolute()
                                                            .right(px(10.))
                                                            .bottom(px(8.))
                                                            .font_family(FONT_MONO)
                                                            .text_size(px(10.))
                                                            .text_color(palette.subtle)
                                                            .child(format!("{:02}", index + 1)),
                                                    )
                                            },
                                        ),
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .when(compact, |rail| rail.w_full())
                            .when(!compact, |rail| rail.w(px(360.)).flex_none())
                            .flex()
                            .flex_col()
                            .gap_4()
                            .child(
                                panel(palette)
                                    .child(section_heading(
                                        IconName::CircleCheck,
                                        "Service health",
                                        "Collector status by surface",
                                        palette,
                                    ))
                                    .child(div().flex().flex_col().children(
                                        self.snapshot.health.iter().map(|health| {
                                            let tone = match health.status.as_str() {
                                                "ok" => Tone::Good,
                                                "warning" | "degraded" => Tone::Warn,
                                                "error" => Tone::Bad,
                                                _ => Tone::Neutral,
                                            };
                                            div()
                                        .py_3()
                                        .border_b_1()
                                        .border_color(palette.border)
                                        .flex()
                                        .items_start()
                                        .gap_3()
                                        .child(div().pt(px(4.)).child(status_dot(tone)))
                                        .child(
                                            div()
                                                .min_w_0()
                                                .flex_grow()
                                                .child(
                                                    div()
                                                        .flex()
                                                        .items_center()
                                                        .justify_between()
                                                        .gap_2()
                                                        .child(
                                                            div()
                                                                .font_weight(
                                                                    gpui::FontWeight::SEMIBOLD,
                                                                )
                                                                .child(health.service.clone()),
                                                        )
                                                        .child(
                                                            div()
                                                                .font_family(FONT_MONO)
                                                                .text_size(px(10.))
                                                                .text_color(tone_color(
                                                                    tone, palette,
                                                                ))
                                                                .child(health.label.to_uppercase()),
                                                        ),
                                                )
                                                .child(
                                                    div()
                                                        .pt_1()
                                                        .text_color(palette.muted)
                                                        .text_size(px(10.))
                                                        .line_height(px(15.))
                                                        .child(health.detail.clone()),
                                                ),
                                        )
                                        }),
                                    )),
                            )
                            .when(!self.recent_changes.is_empty(), |rail| {
                                rail.child(
                                    panel(palette)
                                        .child(section_heading(
                                            IconName::Redo2,
                                            "Snapshot activity",
                                            "Changes since the previous sync",
                                            palette,
                                        ))
                                        .child(activity_stream(
                                            &self.recent_changes,
                                            &activity_time,
                                            palette,
                                            self.reduced_motion,
                                        )),
                                )
                            })
                            .when(self.recent_changes.is_empty(), |rail| {
                                rail.child(
                                    panel(palette)
                                        .child(section_heading(
                                            IconName::Frame,
                                            "Snapshot context",
                                            "Current audit boundary",
                                            palette,
                                        ))
                                        .child(metric_rows(
                                            [
                                                ("Range", self.range.to_string()),
                                                ("Generated", generated_at),
                                                (
                                                    "API calls",
                                                    self.snapshot.collector.api_calls.to_string(),
                                                ),
                                                (
                                                    "Errors",
                                                    self.snapshot.collector.api_errors.to_string(),
                                                ),
                                            ],
                                            palette,
                                        )),
                                )
                            }),
                    ),
            )
            .into_any_element()
    }

    fn filtered_resources(&self, cx: &App, workers_only: bool) -> Vec<ResourceRow> {
        let query = self.search_input.read(cx).value().to_lowercase();
        let investigation_keys = self.investigation.as_ref().and_then(|investigation| {
            (!investigation.resource_keys.is_empty()).then_some(&investigation.resource_keys)
        });
        self.snapshot
            .resources
            .iter()
            .filter(|resource| {
                (!workers_only || resource.kind == "worker")
                    && investigation_keys
                        .is_none_or(|keys| keys.iter().any(|key| key == &resource_key(resource)))
                    && (self.status_filter == "all"
                        || (self.status_filter == "healthy" && resource.status == "healthy")
                        || (self.status_filter == "attention" && resource.status != "healthy"))
                    && (query.is_empty()
                        || resource.name.to_lowercase().contains(query.as_str())
                        || resource.kind.to_lowercase().contains(query.as_str()))
            })
            .cloned()
            .collect()
    }

    fn resource_scope_counts(&self, workers_only: bool) -> (usize, usize, usize) {
        let investigation_keys = self.investigation.as_ref().and_then(|investigation| {
            (!investigation.resource_keys.is_empty()).then_some(&investigation.resource_keys)
        });
        let scoped = self.snapshot.resources.iter().filter(|resource| {
            (!workers_only || resource.kind == "worker")
                && investigation_keys
                    .is_none_or(|keys| keys.iter().any(|key| key == &resource_key(resource)))
        });
        let mut total = 0;
        let mut healthy = 0;
        for resource in scoped {
            total += 1;
            if resource.status == "healthy" {
                healthy += 1;
            }
        }
        (total, healthy, total.saturating_sub(healthy))
    }

    fn render_topology(
        &self,
        layout: Arc<TopologyLayout>,
        workspace_width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.palette();
        if layout.nodes.is_empty() {
            return div()
                .min_h(px(420.))
                .flex_grow()
                .child(empty_resource_state(false, palette))
                .into_any_element();
        }
        let canvas_height = 410.;
        let (scale, offset_x, offset_y) = if self.topology_auto_fit {
            fit_topology_view(&layout, workspace_width, canvas_height)
        } else {
            (
                self.topology_scale,
                self.topology_offset_x,
                self.topology_offset_y,
            )
        };
        let selected_key = self.selected_resource.clone().or_else(|| {
            self.investigation
                .as_ref()
                .and_then(InvestigationContext::current_key)
                .map(str::to_owned)
        });
        let edge_layout = Arc::clone(&layout);
        let edge_selection = selected_key.clone();
        let edge_canvas = canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let spacing = (32. * scale).max(18.);
                let mut grid = PathBuilder::stroke(px(0.5));
                let mut x = offset_x.rem_euclid(spacing);
                while x < f32::from(bounds.size.width) {
                    grid.move_to(point(bounds.origin.x + px(x), bounds.origin.y));
                    grid.line_to(point(
                        bounds.origin.x + px(x),
                        bounds.origin.y + bounds.size.height,
                    ));
                    x += spacing;
                }
                let mut y = offset_y.rem_euclid(spacing);
                while y < f32::from(bounds.size.height) {
                    grid.move_to(point(bounds.origin.x, bounds.origin.y + px(y)));
                    grid.line_to(point(
                        bounds.origin.x + bounds.size.width,
                        bounds.origin.y + px(y),
                    ));
                    y += spacing;
                }
                if let Ok(grid) = grid.build() {
                    window.paint_path(grid, palette.border.opacity(0.34));
                }

                for edge in &edge_layout.edges {
                    let from = &edge_layout.nodes[edge.from];
                    let to = &edge_layout.nodes[edge.to];
                    let from_x = offset_x + from.x * scale + TOPOLOGY_NODE_WIDTH;
                    let from_y = offset_y + from.y * scale + TOPOLOGY_NODE_HEIGHT / 2.;
                    let to_x = offset_x + to.x * scale;
                    let to_y = offset_y + to.y * scale + TOPOLOGY_NODE_HEIGHT / 2.;
                    let bend = ((to_x - from_x).abs() * 0.45).max(48.);
                    let mut path = PathBuilder::stroke(px(
                        if edge_selection.as_deref() == Some(from.key.as_str())
                            || edge_selection.as_deref() == Some(to.key.as_str())
                        {
                            2.
                        } else {
                            1.
                        },
                    ));
                    path.move_to(point(
                        bounds.origin.x + px(from_x),
                        bounds.origin.y + px(from_y),
                    ));
                    path.cubic_bezier_to(
                        point(bounds.origin.x + px(to_x), bounds.origin.y + px(to_y)),
                        point(
                            bounds.origin.x + px(from_x + bend),
                            bounds.origin.y + px(from_y),
                        ),
                        point(
                            bounds.origin.x + px(to_x - bend),
                            bounds.origin.y + px(to_y),
                        ),
                    );
                    if let Ok(path) = path.build() {
                        let selected = edge_selection.as_deref() == Some(from.key.as_str())
                            || edge_selection.as_deref() == Some(to.key.as_str());
                        window.paint_path(
                            path,
                            if selected {
                                palette.accent.opacity(0.82)
                            } else {
                                palette.border_strong.opacity(0.72)
                            },
                        );
                    }
                }
            },
        )
        .absolute()
        .left_0()
        .top_0()
        .size_full();

        let visible_nodes = layout
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| {
                let x = offset_x + node.x * scale;
                let y = offset_y + node.y * scale;
                x > -TOPOLOGY_NODE_WIDTH
                    && x < workspace_width + TOPOLOGY_NODE_WIDTH
                    && y > -TOPOLOGY_NODE_HEIGHT
                    && y < canvas_height + TOPOLOGY_NODE_HEIGHT
            })
            .map(|(index, node)| {
                let key = node.key.clone();
                let selected = selected_key.as_deref() == Some(node.key.as_str());
                let tone = resource_status_tone(&node.status);
                let tone_color = match tone {
                    Tone::Good => palette.good,
                    Tone::Warn => palette.warn,
                    Tone::Bad => palette.bad,
                    Tone::Neutral => palette.subtle,
                };
                div()
                    .id(("topology-node", index))
                    .absolute()
                    .overflow_hidden()
                    .left(px(offset_x + node.x * scale))
                    .top(px(offset_y + node.y * scale))
                    .w(px(TOPOLOGY_NODE_WIDTH))
                    .h(px(TOPOLOGY_NODE_HEIGHT))
                    .px_3()
                    .py_2()
                    .rounded(px(10.))
                    .border_1()
                    .border_color(if selected {
                        palette.accent
                    } else {
                        palette.border
                    })
                    .bg(if selected {
                        palette.accent_soft
                    } else {
                        palette.panel
                    })
                    .cursor_pointer()
                    .hover(move |card| {
                        card.border_color(if selected {
                            palette.accent
                        } else {
                            palette.muted
                        })
                    })
                    .active(|card| card.bg(palette.selected))
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top(px(10.))
                            .bottom(px(10.))
                            .w(px(2.))
                            .rounded_full()
                            .bg(tone_color),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(status_dot(tone))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_grow()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_size(px(12.))
                                    .text_ellipsis()
                                    .child(node.name.clone()),
                            )
                            .child(
                                div()
                                    .size(px(24.))
                                    .rounded(px(6.))
                                    .bg(if selected {
                                        palette.selected
                                    } else {
                                        palette.surface
                                    })
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(Icon::new(resource_kind_icon(&node.kind)).text_color(
                                        if selected {
                                            palette.accent
                                        } else {
                                            palette.muted
                                        },
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .pt_1()
                            .pl(px(16.))
                            .font_family(FONT_MONO)
                            .text_size(px(9.))
                            .text_color(palette.subtle)
                            .child(node.kind.to_uppercase()),
                    )
                    .child(
                        Button::new(SharedString::from(format!("open-topology-node-{index}")))
                            .text()
                            .absolute()
                            .left_0()
                            .right_0()
                            .top_0()
                            .bottom_0()
                            .w_full()
                            .h_full()
                            .cursor_pointer()
                            .tooltip(format!("Inspect {}", node.name))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_resource(key.clone(), true, cx);
                                this.sync_resource_table_selection(cx);
                                cx.stop_propagation();
                            })),
                    )
            })
            .collect::<Vec<_>>();
        let lane_headers = layout
            .lanes
            .iter()
            .enumerate()
            .map(|(index, lane)| {
                div()
                    .id(("topology-lane", index))
                    .absolute()
                    .left(px(offset_x + lane.x * scale))
                    .top(px(offset_y + 18. * scale))
                    .text_size(px(9.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(palette.subtle)
                    .child(lane.label)
            })
            .collect::<Vec<_>>();

        div()
            .min_h_0()
            .flex_grow()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(38.))
                    .flex_none()
                    .px_4()
                    .border_b_1()
                    .border_color(palette.border)
                    .bg(palette.surface.opacity(0.42))
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(palette.muted)
                            .child(format!(
                                "{} resources · {} resolved bindings",
                                layout.nodes.len(),
                                layout.edges.len()
                            )),
                    )
                    .child(
                        div()
                            .ml_auto()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                Button::new("topology-zoom-out")
                                    .icon(IconName::Minus)
                                    .ghost()
                                    .tooltip("Zoom out")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.topology_auto_fit = false;
                                        this.topology_scale = (scale - 0.12).clamp(0.5, 1.6);
                                        this.topology_offset_x = offset_x;
                                        this.topology_offset_y = offset_y;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .w(px(46.))
                                    .text_center()
                                    .font_family(FONT_MONO)
                                    .text_size(px(10.))
                                    .text_color(palette.muted)
                                    .child(format!("{}%", (scale * 100.).round() as usize)),
                            )
                            .child(
                                Button::new("topology-zoom-in")
                                    .icon(IconName::Plus)
                                    .ghost()
                                    .tooltip("Zoom in")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.topology_auto_fit = false;
                                        this.topology_scale = (scale + 0.12).clamp(0.5, 1.6);
                                        this.topology_offset_x = offset_x;
                                        this.topology_offset_y = offset_y;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("topology-fit")
                                    .label("Fit")
                                    .ghost()
                                    .tooltip("Fit topology to workspace")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.topology_auto_fit = true;
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .child(
                div()
                    .id("resource-topology")
                    .relative()
                    .h(px(canvas_height))
                    .flex_none()
                    .w_full()
                    .overflow_hidden()
                    .bg(palette.surface)
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, _| {
                            this.topology_auto_fit = false;
                            this.topology_scale = scale;
                            this.topology_offset_x = offset_x;
                            this.topology_offset_y = offset_y;
                            this.topology_drag = Some((event.position, offset_x, offset_y));
                        }),
                    )
                    .on_mouse_up(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _: &MouseUpEvent, _, _| {
                            this.topology_drag = None;
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                        let Some((start, start_x, start_y)) = this.topology_drag else {
                            return;
                        };
                        if !event.dragging() {
                            this.topology_drag = None;
                            return;
                        }
                        this.topology_offset_x = start_x + f32::from(event.position.x - start.x);
                        this.topology_offset_y = start_y + f32::from(event.position.y - start.y);
                        cx.notify();
                    }))
                    .on_scroll_wheel(cx.listener(move |this, event: &ScrollWheelEvent, _, cx| {
                        let delta_y = match event.delta {
                            ScrollDelta::Pixels(delta) => f32::from(delta.y),
                            ScrollDelta::Lines(delta) => delta.y * 24.,
                        };
                        this.topology_auto_fit = false;
                        this.topology_scale = (scale - delta_y * 0.0015).clamp(0.5, 1.6);
                        this.topology_offset_x = offset_x;
                        this.topology_offset_y = offset_y;
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .child(edge_canvas)
                    .children(lane_headers)
                    .children(visible_nodes),
            )
            .child(
                div()
                    .h(px(34.))
                    .flex_none()
                    .px_4()
                    .border_t_1()
                    .border_color(palette.border)
                    .bg(palette.surface.opacity(0.42))
                    .flex()
                    .items_center()
                    .text_size(px(10.))
                    .text_color(palette.subtle)
                    .child("DRAG TO PAN · WHEEL TO ZOOM · SELECT A NODE TO INSPECT"),
            )
            .into_any_element()
    }

    fn render_resources(
        &mut self,
        workers_only: bool,
        compact: bool,
        workspace_width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.palette();
        let resources = self.filtered_resources(cx, workers_only);
        let resource_count = resources.len();
        let (scope_count, healthy_count, attention_count) =
            self.resource_scope_counts(workers_only);
        let has_search = !self.search_input.read(cx).value().trim().is_empty();
        let filters_active = has_search || self.status_filter != "all";
        let mut fingerprint =
            resource_table_fingerprint(&resources, workers_only, self.status_filter, self.dark);
        if let Some(current) = self
            .investigation
            .as_ref()
            .and_then(InvestigationContext::current_key)
        {
            fingerprint.push_str("\u{1f}investigation:");
            fingerprint.push_str(current);
        }
        if !workers_only
            && self.resource_view_mode == ResourceViewMode::Topology
            && self.topology_fingerprint != fingerprint
        {
            self.topology_layout = Arc::new(build_topology_layout(&resources));
            self.topology_fingerprint = fingerprint.clone();
            self.topology_auto_fit = true;
        }
        let selected_key = self.selected_resource.clone();
        let investigation_keys = self
            .investigation
            .as_ref()
            .map(|investigation| investigation.resource_keys.iter().cloned().collect())
            .unwrap_or_default();
        let investigation_current = self
            .investigation
            .as_ref()
            .and_then(InvestigationContext::current_key)
            .map(str::to_owned);
        self.resource_table.update(cx, |table, cx| {
            table.delegate_mut().selected_key = selected_key.clone();
            table.delegate_mut().filters_active = filters_active;
            let layout_changed = table.delegate_mut().set_compact(compact);
            if table.delegate_mut().sync(
                resources.clone(),
                fingerprint.clone(),
                workers_only,
                palette,
                investigation_keys,
                investigation_current,
            ) || layout_changed
            {
                let selected_row = selected_key.as_ref().and_then(|selected_key| {
                    table
                        .delegate()
                        .rows
                        .iter()
                        .position(|resource| resource_key(resource) == *selected_key)
                });
                table.clear_selection(cx);
                table.refresh(cx);
                if let Some(selected_row) = selected_row {
                    table.set_selected_row(selected_row, cx);
                }
            }
        });
        let table_focus = self.resource_table.focus_handle(cx);
        let workspace_content =
            if workers_only || self.resource_view_mode == ResourceViewMode::Table {
                div()
                    .min_h(px(410.))
                    .min_w_0()
                    .flex_grow()
                    .on_mouse_down(gpui::MouseButton::Left, move |_, window, _| {
                        table_focus.focus(window);
                    })
                    .child(
                        Table::new(&self.resource_table)
                            .large()
                            .bordered(false)
                            .stripe(false),
                    )
                    .into_any_element()
            } else {
                self.render_topology(Arc::clone(&self.topology_layout), workspace_width, cx)
            };
        let workspace_content = div()
            .min_h_0()
            .flex_grow()
            .flex()
            .flex_col()
            .child(workspace_content)
            .with_animation(
                SharedString::from(format!(
                    "resource-view-{}",
                    if workers_only {
                        "workers"
                    } else if self.resource_view_mode == ResourceViewMode::Topology {
                        "topology"
                    } else {
                        "table"
                    }
                )),
                Animation::new(motion_duration(self.reduced_motion, 140))
                    .with_easing(ease_out_quint()),
                |view, delta| view.opacity(delta),
            );

        div()
            .min_h_0()
            .flex_grow()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .min_h_0()
                    .flex_grow()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .rounded(px(12.))
                    .border_1()
                    .border_color(palette.border)
                    .bg(palette.panel)
                    .child(
                        div()
                            .px_5()
                            .pt_4()
                            .pb_3()
                            .border_b_1()
                            .border_color(palette.border)
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_4()
                            .child(
                                div()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .text_size(px(15.))
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .child(if workers_only {
                                                "Worker inventory"
                                            } else {
                                                "Inventory workspace"
                                            }),
                                    )
                                    .child(
                                        div()
                                            .pt_1()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .text_size(px(10.))
                                            .text_color(palette.muted)
                                            .child(format!(
                                                "{resource_count} shown of {scope_count}"
                                            ))
                                            .when(filters_active, |meta| {
                                                meta.child(
                                                    div()
                                                        .font_family(FONT_MONO)
                                                        .text_size(px(10.))
                                                        .text_color(palette.accent)
                                                        .child("FILTERED"),
                                                )
                                            }),
                                    ),
                            )
                            .when(!workers_only, |header| {
                                header.child(
                                    div()
                                        .p_1()
                                        .rounded(px(8.))
                                        .border_1()
                                        .border_color(palette.border)
                                        .bg(palette.surface)
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .child(
                                            Button::new("resources-table-view")
                                                .icon(IconName::Frame)
                                                .label("Table")
                                                .tooltip("Resource table")
                                                .when(
                                                    self.resource_view_mode
                                                        == ResourceViewMode::Table,
                                                    |button| {
                                                        button
                                                            .bg(palette.selected)
                                                            .text_color(palette.accent)
                                                    },
                                                )
                                                .when(
                                                    self.resource_view_mode
                                                        != ResourceViewMode::Table,
                                                    |button| button.ghost(),
                                                )
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.resource_view_mode =
                                                        ResourceViewMode::Table;
                                                    cx.notify();
                                                })),
                                        )
                                        .child(
                                            Button::new("resources-topology-view")
                                                .icon(IconName::Inspector)
                                                .label("Topology")
                                                .tooltip("Resource topology")
                                                .when(
                                                    self.resource_view_mode
                                                        == ResourceViewMode::Topology,
                                                    |button| {
                                                        button
                                                            .bg(palette.selected)
                                                            .text_color(palette.accent)
                                                    },
                                                )
                                                .when(
                                                    self.resource_view_mode
                                                        != ResourceViewMode::Topology,
                                                    |button| button.ghost(),
                                                )
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.resource_view_mode =
                                                        ResourceViewMode::Topology;
                                                    cx.notify();
                                                })),
                                        ),
                                )
                            }),
                    )
                    .child(
                        div()
                            .px_4()
                            .py_3()
                            .border_b_1()
                            .border_color(palette.border)
                            .bg(palette.surface.opacity(0.42))
                            .flex()
                            .items_center()
                            .when(compact, |toolbar| toolbar.flex_wrap())
                            .gap_2()
                            .child(
                                div()
                                    .min_w(px(220.))
                                    .flex_grow()
                                    .when(!compact, |search| search.max_w(px(360.)))
                                    .when(compact, |search| search.w_full())
                                    .child(Input::new(&self.search_input).cleanable(true)),
                            )
                            .child(
                                div().flex().items_center().gap_1().children(
                                    [
                                        ("all", format!("All {scope_count}")),
                                        ("healthy", format!("Healthy {healthy_count}")),
                                        ("attention", format!("Attention {attention_count}")),
                                    ]
                                    .into_iter()
                                    .map(|(filter, label)| {
                                        Button::new(SharedString::from(format!("filter-{filter}")))
                                            .label(label)
                                            .when(self.status_filter == filter, |button| {
                                                button
                                                    .bg(palette.selected)
                                                    .text_color(palette.accent)
                                            })
                                            .when(self.status_filter != filter, |button| {
                                                button.ghost()
                                            })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.status_filter = filter;
                                                cx.notify();
                                            }))
                                    }),
                                ),
                            )
                            .when(filters_active, |toolbar| {
                                toolbar.child(
                                    Button::new("clear-resource-filters")
                                        .ghost()
                                        .icon(IconName::Close)
                                        .tooltip("Clear search and status filter")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.status_filter = "all";
                                            this.clear_resource_search(window, cx);
                                            cx.notify();
                                        })),
                                )
                            }),
                    )
                    .child(workspace_content)
                    .when(
                        workers_only || self.resource_view_mode == ResourceViewMode::Table,
                        |workspace| {
                            workspace.child(
                                div()
                                    .h(px(34.))
                                    .flex_none()
                                    .px_4()
                                    .border_t_1()
                                    .border_color(palette.border)
                                    .bg(palette.surface.opacity(0.42))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .font_family(FONT_MONO)
                                    .text_size(px(10.))
                                    .text_color(palette.subtle)
                                    .child(format!("{resource_count} VISIBLE"))
                                    .when(!compact, |footer| {
                                        footer.child(
                                            "SELECT A ROW TO INSPECT · ↑↓ NAVIGATE · CTRL/CMD F SEARCH",
                                        )
                                    }),
                            )
                        },
                    ),
            )
            .when(workers_only, |view| {
                view.child(self.render_observability_summary(compact))
            })
            .into_any_element()
    }

    fn render_observability_summary(&self, compact: bool) -> AnyElement {
        let palette = self.palette();
        let obs = &self.snapshot.observability;
        div()
            .child(section_heading(
                IconName::Eye,
                "Workers observability",
                "Telemetry and configuration coverage",
                palette,
            ))
            .child(
                div()
                    .flex()
                    .when(compact, |stats| stats.flex_wrap())
                    .gap_3()
                    .children(
                        [
                            ("Log events", compact_number(obs.log_events)),
                            ("Traces", compact_number(obs.traces)),
                            ("Configured", obs.configured_workers.to_string()),
                            ("Destinations", obs.destinations.to_string()),
                        ]
                        .into_iter()
                        .map(|(label, value)| {
                            stat_card(label, value, "Selected range", palette)
                                .min_w(px(if compact { 180. } else { 0. }))
                                .flex_1()
                        }),
                    ),
            )
            .into_any_element()
    }

    fn render_billing(&self, compact: bool, wide: bool) -> AnyElement {
        let palette = self.palette();
        let metrics = &self.snapshot.metrics;
        let stats = [
            (
                "Projection",
                metrics.cost_usd.map(money).unwrap_or_else(|| "N/A".into()),
                "Current usage",
            ),
            (
                "Base",
                metrics
                    .cost_base_usd
                    .map(money)
                    .unwrap_or_else(|| "N/A".into()),
                "Workers Paid",
            ),
            (
                "Overage",
                metrics
                    .cost_overage_usd
                    .map(money)
                    .unwrap_or_else(|| "$0.00".into()),
                "Projected",
            ),
            (
                "Currency",
                metrics
                    .cost_currency
                    .clone()
                    .unwrap_or_else(|| "USD".into()),
                metrics
                    .cost_source
                    .as_deref()
                    .unwrap_or("Analytics-derived"),
            ),
        ];
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(metrics_strip(stats, palette))
            .child(
                div()
                    .grid()
                    .grid_cols(12)
                    .gap_4()
                    .child(
                        panel(palette)
                            .when(compact, |panel| {
                                panel.row_start(1).col_start(1).col_span_full()
                            })
                            .when(!compact && !wide, |panel| {
                                panel.row_start(1).col_start(1).col_span(7)
                            })
                            .when(wide, |panel| panel.row_start(1).col_start(1).col_span(8))
                            .child(section_heading(
                                IconName::ChartPie,
                                "Usage drivers",
                                "Cloudflare analytics, not invoice-grade billing",
                                palette,
                            ))
                            .child(metric_rows(
                                [
                                    ("Worker requests", compact_number(metrics.worker_requests)),
                                    (
                                        "Worker CPU time",
                                        metrics
                                            .worker_cpu_time_ms
                                            .map(|value| format!("{value:.1} ms"))
                                            .unwrap_or_else(|| "N/A".into()),
                                    ),
                                    ("D1 queries", compact_number(metrics.d1_queries)),
                                    ("R2 operations", compact_number(metrics.r2_operations)),
                                    ("R2 storage", format_bytes(metrics.r2_storage_bytes)),
                                    ("KV operations", compact_number(metrics.kv_operations)),
                                    ("KV storage", format_bytes(metrics.kv_storage_bytes)),
                                ],
                                palette,
                            )),
                    )
                    .child(
                        panel(palette)
                            .when(compact, |panel| {
                                panel.row_start(2).col_start(1).col_span_full()
                            })
                            .when(!compact && !wide, |panel| {
                                panel.row_start(1).col_start(8).col_span(5)
                            })
                            .when(wide, |panel| panel.row_start(1).col_start(9).col_span(4))
                            .child(section_heading(
                                IconName::Info,
                                "Projection basis",
                                "How Cedar framed this estimate",
                                palette,
                            ))
                            .child(metric_rows(
                                [
                                    (
                                        "Method",
                                        metrics
                                            .cost_source
                                            .clone()
                                            .unwrap_or_else(|| "Analytics-derived".into()),
                                    ),
                                    ("Plan", "Workers Paid".into()),
                                    ("Range", self.range.into()),
                                    (
                                        "Availability",
                                        if metrics.cost_usd.is_some() {
                                            "Projection ready".into()
                                        } else {
                                            "Awaiting analytics".into()
                                        },
                                    ),
                                ],
                                palette,
                            )),
                    ),
            )
            .into_any_element()
    }

    fn render_settings(&self, compact: bool, cx: &mut Context<Self>) -> AnyElement {
        let palette = self.palette();
        let account_name = self
            .connection
            .as_ref()
            .and_then(|connection| connection.account.as_ref())
            .map(|account| account.name.clone())
            .unwrap_or_else(|| "No account connected".into());
        let (update_icon, update_detail, update_action) = match &self.update_status {
            UpdateStatus::Idle => (
                IconName::Redo2,
                "Ready to check GitHub Releases".to_string(),
                Button::new("settings-check-update")
                    .ghost()
                    .icon(IconName::Redo2)
                    .label("Check now")
                    .tooltip("Check GitHub Releases for a newer Cedar version")
                    .on_click(cx.listener(Self::check_for_updates_click))
                    .into_any_element(),
            ),
            UpdateStatus::Checking => (
                IconName::LoaderCircle,
                "Checking GitHub Releases…".to_string(),
                Button::new("settings-checking-update")
                    .ghost()
                    .label("Checking")
                    .loading(!self.reduced_motion)
                    .disabled(true)
                    .into_any_element(),
            ),
            UpdateStatus::UpToDate => (
                IconName::CircleCheck,
                format!("Cedar {} is up to date", env!("CARGO_PKG_VERSION")),
                Button::new("settings-recheck-update")
                    .ghost()
                    .icon(IconName::Redo2)
                    .label("Check again")
                    .on_click(cx.listener(Self::check_for_updates_click))
                    .into_any_element(),
            ),
            UpdateStatus::Available(update) => {
                let published = update
                    .published_at
                    .as_deref()
                    .map(|date| format!(" · {}", date.split('T').next().unwrap_or(date)))
                    .unwrap_or_default();
                let notes = update
                    .notes
                    .lines()
                    .find(|line| !line.trim().is_empty())
                    .map(|line| format!(" · {}", line.trim()))
                    .unwrap_or_default();
                (
                    IconName::ArrowDown,
                    format!("Cedar {} is available{published}{notes}", update.version),
                    Button::new("settings-download-update")
                        .primary()
                        .icon(IconName::ArrowDown)
                        .label("Download")
                        .tooltip(format!("Download and verify {}", update.asset_name))
                        .on_click(cx.listener(Self::download_update_click))
                        .into_any_element(),
                )
            }
            UpdateStatus::Downloading(update) => (
                IconName::LoaderCircle,
                format!("Downloading and verifying Cedar {}…", update.version),
                Button::new("settings-downloading-update")
                    .ghost()
                    .label("Downloading")
                    .loading(!self.reduced_motion)
                    .disabled(true)
                    .into_any_element(),
            ),
            UpdateStatus::Ready(download) => (
                IconName::CircleCheck,
                format!(
                    "Cedar {} is ready · {}",
                    download.info.version, download.verification
                ),
                Button::new("settings-install-update")
                    .primary()
                    .icon(IconName::Redo2)
                    .label("Restart to update")
                    .tooltip("Restart Cedar and install the verified update")
                    .on_click(cx.listener(Self::install_update_click))
                    .into_any_element(),
            ),
            UpdateStatus::Installing => (
                IconName::LoaderCircle,
                "Preparing the verified update and restarting…".to_string(),
                Button::new("settings-installing-update")
                    .ghost()
                    .label("Restarting")
                    .loading(!self.reduced_motion)
                    .disabled(true)
                    .into_any_element(),
            ),
            UpdateStatus::Error(error) => (
                IconName::TriangleAlert,
                error.clone(),
                Button::new("settings-retry-update")
                    .ghost()
                    .icon(IconName::Redo2)
                    .label("Retry")
                    .on_click(cx.listener(Self::check_for_updates_click))
                    .into_any_element(),
            ),
        };

        div()
            .grid()
            .grid_cols(12)
            .gap_4()
            .child(
                panel(palette)
                    .when(compact, |panel| {
                        panel.row_start(1).col_start(1).col_span_full()
                    })
                    .when(!compact, |panel| {
                        panel.row_start(1).col_start(1).col_span(7)
                    })
                    .child(section_heading(
                        IconName::Palette,
                        "Appearance",
                        "Tune Cedar without changing its monochrome surfaces",
                        palette,
                    ))
                    .child(settings_preference_row(
                        if self.dark {
                            IconName::Moon
                        } else {
                            IconName::Sun
                        },
                        "Interface theme",
                        if self.dark {
                            "Dark monochrome is active"
                        } else {
                            "Light monochrome is active"
                        },
                        Button::new("settings-theme")
                            .ghost()
                            .icon(if self.dark {
                                IconName::Sun
                            } else {
                                IconName::Moon
                            })
                            .label(if self.dark { "Use light" } else { "Use dark" })
                            .tooltip("Switch Cedar's interface theme")
                            .on_click(cx.listener(Self::toggle_theme))
                            .into_any_element(),
                        palette,
                    ))
                    .child(settings_preference_row(
                        IconName::Redo2,
                        "Interface motion",
                        if self.reduced_motion {
                            "Transitions are reduced"
                        } else {
                            "Short interface transitions are enabled"
                        },
                        Button::new("settings-motion")
                            .ghost()
                            .icon(if self.reduced_motion {
                                IconName::CircleCheck
                            } else {
                                IconName::Redo2
                            })
                            .label(if self.reduced_motion {
                                "Use full motion"
                            } else {
                                "Reduce motion"
                            })
                            .tooltip("Change Cedar's motion preference")
                            .on_click(cx.listener(Self::toggle_reduced_motion))
                            .into_any_element(),
                        palette,
                    )),
            )
            .child(
                panel(palette)
                    .when(compact, |panel| {
                        panel.row_start(2).col_start(1).col_span_full()
                    })
                    .when(!compact, |panel| {
                        panel.row_start(1).col_start(8).col_span(5)
                    })
                    .child(section_heading(
                        IconName::PanelLeft,
                        "Navigation",
                        "Sidebar state and account access",
                        palette,
                    ))
                    .child(settings_preference_row(
                        if self.sidebar_collapsed {
                            IconName::PanelLeftOpen
                        } else {
                            IconName::PanelLeftClose
                        },
                        "Sidebar",
                        if self.sidebar_collapsed {
                            "Compact navigation is active · Ctrl/Cmd B"
                        } else {
                            "Expanded navigation is active · Ctrl/Cmd B"
                        },
                        Button::new("settings-sidebar")
                            .ghost()
                            .icon(if self.sidebar_collapsed {
                                IconName::PanelLeftOpen
                            } else {
                                IconName::PanelLeftClose
                            })
                            .label(if self.sidebar_collapsed {
                                "Expand"
                            } else {
                                "Collapse"
                            })
                            .tooltip("Toggle the Cedar sidebar")
                            .on_click(cx.listener(Self::toggle_sidebar_click))
                            .into_any_element(),
                        palette,
                    ))
                    .child(settings_preference_row(
                        IconName::Globe,
                        "Cloudflare account",
                        &account_name,
                        Button::new("settings-open-connection")
                            .ghost()
                            .icon(IconName::ArrowRight)
                            .label("Open")
                            .tooltip("Open connection settings")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.active_section = Section::Connection;
                                cx.notify();
                            }))
                            .into_any_element(),
                        palette,
                    )),
            )
            .child(
                panel(palette)
                    .row_start(if compact { 3 } else { 2 })
                    .col_start(1)
                    .col_span_full()
                    .child(section_heading(
                        IconName::Info,
                        "Application",
                        "Cedar stays native and local-first",
                        palette,
                    ))
                    .child(metric_rows(
                        [
                            ("Interface", "Native GPUI".into()),
                            ("Runtime", "Rust".into()),
                            ("Snapshots", "Local SQLite".into()),
                            ("Version", env!("CARGO_PKG_VERSION").into()),
                        ],
                        palette,
                    ))
                    .child(settings_preference_row(
                        update_icon,
                        "Software update",
                        &update_detail,
                        update_action,
                        palette,
                    ))
                    .child(settings_preference_row(
                        if self.automatic_update_checks {
                            IconName::CircleCheck
                        } else {
                            IconName::CircleX
                        },
                        "Automatic checks",
                        if self.automatic_update_checks {
                            "Check for a new release when Cedar starts"
                        } else {
                            "Only check when requested"
                        },
                        Button::new("settings-automatic-updates")
                            .ghost()
                            .icon(if self.automatic_update_checks {
                                IconName::CircleCheck
                            } else {
                                IconName::CircleX
                            })
                            .label(if self.automatic_update_checks {
                                "Enabled"
                            } else {
                                "Disabled"
                            })
                            .tooltip("Toggle automatic update checks")
                            .on_click(cx.listener(Self::toggle_automatic_update_checks))
                            .into_any_element(),
                        palette,
                    )),
            )
            .into_any_element()
    }

    fn render_connection(&self, compact: bool, wide: bool, cx: &mut Context<Self>) -> AnyElement {
        let palette = self.palette();
        if !self.connected() {
            let has_token = !self.token_input.read(cx).unmask_value().trim().is_empty();
            let token_verified = !self.accounts.is_empty();
            let selected_account = self.selected_account_id.as_deref();
            let setup = panel(palette)
                .when(compact, |panel| panel.row_start(1).col_start(1).col_span_full())
                .when(!compact && !wide, |panel| {
                    panel.row_start(1).col_start(1).col_span(7)
                })
                .when(wide, |panel| panel.row_start(1).col_start(1).col_span(8))
                .child(
                    div()
                        .pb_5()
                        .flex()
                        .items_center()
                        .child(connection_step(
                            "01",
                            "Authenticate",
                            "Verify the API token",
                            token_verified,
                            !token_verified,
                            palette,
                        ))
                        .child(
                            div()
                                .mx_3()
                                .h(px(1.))
                                .flex_grow()
                                .bg(if token_verified {
                                    palette.accent
                                } else {
                                    palette.border
                                }),
                        )
                        .child(connection_step(
                            "02",
                            "Choose account",
                            "Confirm the audit target",
                            false,
                            token_verified,
                            palette,
                        )),
                )
                .when(!token_verified, |view| {
                    view.child(section_heading(
                        IconName::CircleCheck,
                        "Cloudflare API token",
                        "Cedar verifies the token before storing it",
                        palette,
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(
                                Input::new(&self.token_input)
                                    .mask_toggle()
                                    .disabled(self.syncing),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_size(px(10.))
                                    .text_color(if has_token {
                                        palette.muted
                                    } else {
                                        palette.subtle
                                    })
                                    .child(status_dot(if has_token {
                                        Tone::Neutral
                                    } else {
                                        Tone::Warn
                                    }))
                                    .child(if has_token {
                                        "Ready to verify with Cloudflare"
                                    } else {
                                        "Paste a token to continue"
                                    }),
                            )
                            .child(
                                div()
                                    .pt_2()
                                    .flex()
                                    .flex_wrap()
                                    .gap_2()
                                    .child(
                                        Button::new("verify-cloudflare-token")
                                            .primary()
                                            .icon(IconName::CircleCheck)
                                            .label(if self.syncing {
                                                "Verifying…"
                                            } else {
                                                "Verify token"
                                            })
                                            .loading(self.syncing && !self.reduced_motion)
                                            .disabled(self.syncing || !has_token)
                                            .on_click(cx.listener(Self::discover_accounts)),
                                    )
                                    .child(
                                        Button::new("token-template")
                                            .ghost()
                                            .icon(IconName::ExternalLink)
                                            .label("Create token")
                                            .on_click(|_, _, cx| {
                                                cx.open_url(
                                                    "https://dash.cloudflare.com/profile/api-tokens",
                                                )
                                            }),
                                    ),
                            ),
                    )
                })
                .when(token_verified, |view| {
                    view.child(
                        div()
                            .mb_5()
                            .p_3()
                            .rounded(px(9.))
                            .border_1()
                            .border_color(palette.good.opacity(0.42))
                            .bg(palette.good.opacity(0.08))
                            .flex()
                            .items_center()
                            .gap_3()
                            .text_color(palette.good)
                            .child(Icon::new(IconName::CircleCheck))
                            .child(
                                div()
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .child("Token verified"),
                                    )
                                    .child(
                                        div()
                                            .pt_1()
                                            .text_size(px(10.))
                                            .text_color(palette.muted)
                                            .child(format!(
                                                "Cloudflare returned {} account{}",
                                                self.accounts.len(),
                                                if self.accounts.len() == 1 { "" } else { "s" }
                                            )),
                                    ),
                            ),
                    )
                    .child(section_heading(
                        IconName::Building2,
                        "Choose the audit account",
                        "Cedar stores only the selected account locally",
                        palette,
                    ))
                    .child(
                        div().flex().flex_col().gap_2().children(self.accounts.iter().map(
                            |account| {
                                let id = account.id.clone();
                                let button_account_id = id.clone();
                                let selected = selected_account == Some(id.as_str());
                                div()
                                    .id(SharedString::from(format!("account-choice-{id}")))
                                    .p_3()
                                    .rounded(px(9.))
                                    .border_1()
                                    .border_color(if selected {
                                        palette.accent
                                    } else {
                                        palette.border
                                    })
                                    .bg(if selected {
                                        palette.selected
                                    } else {
                                        palette.surface
                                    })
                                    .cursor_pointer()
                                    .hover(move |row| {
                                        row.border_color(palette.border_strong).bg(if selected {
                                            palette.accent_soft
                                        } else {
                                            palette.hover
                                        })
                                    })
                                    .active(|row| row.bg(palette.selected))
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .child(
                                        div()
                                            .size(px(34.))
                                            .flex_none()
                                            .rounded(px(9.))
                                            .bg(palette.panel)
                                            .text_color(if selected {
                                                palette.accent
                                            } else {
                                                palette.muted
                                            })
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(Icon::new(IconName::Building2)),
                                    )
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex_grow()
                                            .child(
                                                div()
                                                    .text_size(px(12.))
                                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                                    .child(account.name.clone()),
                                            )
                                            .child(
                                                div()
                                                    .pt_1()
                                                    .font_family(FONT_MONO)
                                                    .text_size(px(10.))
                                                    .text_color(palette.subtle)
                                                    .text_ellipsis()
                                                    .child(account.id.clone()),
                                            ),
                                    )
                                    .child(
                                        Button::new(SharedString::from(format!(
                                            "select-account-{button_account_id}"
                                        )))
                                        .ghost()
                                        .compact()
                                        .icon(if selected {
                                            IconName::Check
                                        } else {
                                            IconName::ArrowRight
                                        })
                                        .label(if selected { "Selected" } else { "Select" })
                                        .tooltip("Select this Cloudflare account")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.selected_account_id =
                                                Some(button_account_id.clone());
                                            cx.notify();
                                        })),
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.selected_account_id = Some(id.clone());
                                        cx.notify();
                                    }))
                            },
                        )),
                    )
                    .child(
                        div()
                            .pt_5()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(
                                Button::new("connect-selected-account")
                                    .primary()
                                    .icon(IconName::ArrowRight)
                                    .label(if self.syncing {
                                        "Connecting…"
                                    } else {
                                        "Connect selected account"
                                    })
                                    .loading(self.syncing && !self.reduced_motion)
                                    .disabled(
                                        self.syncing || self.selected_account_id.is_none(),
                                    )
                                    .on_click(cx.listener(Self::connect)),
                            )
                            .child(
                                Button::new("change-cloudflare-token")
                                    .ghost()
                                    .label("Change token")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.accounts.clear();
                                        this.selected_account_id = None;
                                        this.token_input
                                            .update(cx, |input, cx| input.focus(window, cx));
                                        cx.notify();
                                    })),
                            ),
                    )
                });

            let scopes = panel(palette)
                .when(compact, |panel| panel.row_start(2).col_start(1).col_span_full())
                .when(!compact && !wide, |panel| {
                    panel.row_start(1).col_start(8).col_span(5)
                })
                .when(wide, |panel| panel.row_start(1).col_start(9).col_span(4))
                .child(section_heading(
                    IconName::CircleCheck,
                    "Access checklist",
                    "Permissions Cedar expects from the token",
                    palette,
                ))
                .child(connection_scope(
                    "Core audit",
                    "Account, analytics, Workers, Pages, zones, D1, R2, KV, and Audit Logs",
                    "10 READ SCOPES",
                    true,
                    palette,
                ))
                .child(connection_scope(
                    "Observability",
                    "Logpush inventory and Workers telemetry configuration",
                    "2 OPTIONAL SCOPES",
                    false,
                    palette,
                ))
                .child(
                    div()
                        .pt_4()
                        .text_size(px(10.))
                        .line_height(px(16.))
                        .text_color(palette.muted)
                        .child(
                            "Optional gaps do not block the audit. Cedar reports unavailable collectors after the first sync.",
                        ),
                );

            return div()
                .grid()
                .grid_cols(12)
                .gap_4()
                .child(setup)
                .child(scopes)
                .child(
                    div()
                        .row_start(if compact { 3 } else { 2 })
                        .col_start(1)
                        .col_span_full()
                        .px_4()
                        .py_3()
                        .rounded(px(10.))
                        .border_1()
                        .border_color(palette.border)
                        .bg(palette.surface)
                        .flex()
                        .when(compact, |strip| strip.flex_wrap())
                        .gap_4()
                        .child(connection_trust_item(
                            IconName::CircleCheck,
                            "Credential",
                            "OS keychain",
                            palette,
                        ))
                        .child(connection_trust_item(
                            IconName::Frame,
                            "Snapshots",
                            "Local SQLite",
                            palette,
                        ))
                        .child(connection_trust_item(
                            IconName::Eye,
                            "Cloud access",
                            "Read-only audit",
                            palette,
                        )),
                )
                .into_any_element();
        }

        let connection = self.connection.as_ref();
        let account = connection.and_then(|value| value.account.as_ref());
        let account_name = account
            .map(|account| account.name.clone())
            .unwrap_or_else(|| "Unknown account".into());
        let account_id = account
            .map(|account| account.id.clone())
            .unwrap_or_else(|| "No account ID".into());
        let copied_account_id = account_id.clone();
        let required_failures = self
            .snapshot
            .collector
            .endpoints
            .iter()
            .filter(|endpoint| !endpoint.ok && !endpoint.optional)
            .count();
        let optional_gaps = self
            .snapshot
            .collector
            .endpoints
            .iter()
            .filter(|endpoint| !endpoint.ok && endpoint.optional)
            .count();
        let has_diagnostics = !self.snapshot.collector.endpoints.is_empty();
        let observability_reported = self.snapshot.observability.log_events > 0
            || self.snapshot.observability.traces > 0
            || self.snapshot.observability.configured_workers > 0
            || self.snapshot.observability.destinations > 0;
        let diagnostics_detail = format!(
            "{} calls · {} blocking errors · p95 {}",
            self.snapshot.collector.api_calls,
            self.snapshot.collector.api_errors,
            self.snapshot
                .collector
                .api_duration_p95_ms
                .map(|duration| format!("{duration:.0} ms"))
                .unwrap_or_else(|| "—".into())
        );

        div()
            .grid()
            .grid_cols(12)
            .gap_4()
            .child(
                panel(palette)
                    .when(compact, |panel| {
                        panel.row_start(1).col_start(1).col_span_full()
                    })
                    .when(!compact && !wide, |panel| {
                        panel.row_start(1).col_start(1).col_span(7)
                    })
                    .when(wide, |panel| panel.row_start(1).col_start(1).col_span(8))
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap_4()
                            .child(
                                div()
                                    .size(px(46.))
                                    .flex_none()
                                    .rounded(px(13.))
                                    .bg(palette.accent_soft)
                                    .text_color(palette.accent)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(Icon::new(IconName::Building2)),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_grow()
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .child(status_dot(Tone::Good))
                                            .child(
                                                div()
                                                    .font_family(FONT_MONO)
                                                    .text_size(px(10.))
                                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                                    .text_color(palette.good)
                                                    .child("CONNECTION VERIFIED"),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .pt_2()
                                            .text_size(px(19.))
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .child(account_name),
                                    )
                                    .child(
                                        div()
                                            .pt_1()
                                            .font_family(FONT_MONO)
                                            .text_size(px(9.))
                                            .text_color(palette.subtle)
                                            .text_ellipsis()
                                            .child(account_id),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .pt_5()
                            .grid()
                            .grid_cols(3)
                            .gap_3()
                            .child(connection_metric(
                                "Last snapshot",
                                if self.snapshot.generated_at.is_empty() {
                                    "Not synced".into()
                                } else {
                                    self.snapshot.generated_at.clone()
                                },
                                palette,
                            ))
                            .child(connection_metric(
                                "API calls",
                                self.snapshot.collector.api_calls.to_string(),
                                palette,
                            ))
                            .child(connection_metric(
                                "Rate limit",
                                self.snapshot
                                    .collector
                                    .rate_limit_remaining
                                    .clone()
                                    .unwrap_or_else(|| "Not reported".into()),
                                palette,
                            )),
                    )
                    .child(
                        div()
                            .pt_5()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(
                                Button::new("settings-sync-now")
                                    .primary()
                                    .icon(IconName::Redo2)
                                    .label(if self.syncing {
                                        "Syncing…"
                                    } else {
                                        "Sync now"
                                    })
                                    .loading(self.syncing && !self.reduced_motion)
                                    .disabled(self.syncing)
                                    .on_click(cx.listener(|this, _, _, cx| this.refresh(true, cx))),
                            )
                            .child(
                                Button::new("copy-account-id")
                                    .ghost()
                                    .icon(IconName::Copy)
                                    .label("Copy account ID")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            copied_account_id.clone(),
                                        ));
                                        this.show_notice("Account ID copied", false, window, cx);
                                    })),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .when(!self.disconnect_confirmation_open, |view| {
                                        view.child(
                                            Button::new("disconnect")
                                                .danger()
                                                .label("Disconnect")
                                                .on_click(cx.listener(
                                                    Self::toggle_disconnect_confirmation,
                                                )),
                                        )
                                    })
                                    .when(self.disconnect_confirmation_open, |view| {
                                        view.child(
                                            div()
                                                .text_size(px(11.))
                                                .text_color(palette.muted)
                                                .child("Remove this account from Cedar?"),
                                        )
                                        .child(
                                            Button::new("cancel-disconnect")
                                                .ghost()
                                                .compact()
                                                .label("Cancel")
                                                .on_click(cx.listener(
                                                    Self::toggle_disconnect_confirmation,
                                                )),
                                        )
                                        .child(
                                            Button::new("confirm-disconnect")
                                                .danger()
                                                .compact()
                                                .label("Confirm")
                                                .on_click(cx.listener(Self::clear_connection)),
                                        )
                                    }),
                            ),
                    ),
            )
            .child(
                panel(palette)
                    .when(compact, |panel| {
                        panel.row_start(2).col_start(1).col_span_full()
                    })
                    .when(!compact && !wide, |panel| {
                        panel.row_start(1).col_start(8).col_span(5)
                    })
                    .when(wide, |panel| panel.row_start(1).col_start(9).col_span(4))
                    .child(section_heading(
                        IconName::Eye,
                        "Access coverage",
                        "What the latest sync could read",
                        palette,
                    ))
                    .child(connection_status_row(
                        "Core audit",
                        if !has_diagnostics {
                            "Awaiting sync".into()
                        } else if required_failures == 0 {
                            "Available".into()
                        } else {
                            format!("{required_failures} blocked")
                        },
                        if !has_diagnostics {
                            Tone::Neutral
                        } else if required_failures == 0 {
                            Tone::Good
                        } else {
                            Tone::Bad
                        },
                        palette,
                    ))
                    .child(connection_status_row(
                        "Optional telemetry",
                        if !has_diagnostics {
                            "Awaiting sync".into()
                        } else if optional_gaps > 0 {
                            format!("{optional_gaps} limited")
                        } else if observability_reported {
                            "Reported".into()
                        } else {
                            "No signals".into()
                        },
                        if optional_gaps > 0 {
                            Tone::Warn
                        } else if observability_reported {
                            Tone::Good
                        } else {
                            Tone::Neutral
                        },
                        palette,
                    ))
                    .child(connection_status_row(
                        "Credential",
                        if connection.is_some_and(|value| value.token_present) {
                            connection
                                .map(|value| value.storage.to_string())
                                .unwrap_or_else(|| "OS keychain".into())
                        } else {
                            "Missing".into()
                        },
                        if connection.is_some_and(|value| value.token_present) {
                            Tone::Good
                        } else {
                            Tone::Bad
                        },
                        palette,
                    ))
                    .child(connection_status_row(
                        "Snapshot source",
                        if self.snapshot.live {
                            "Live Cloudflare data".into()
                        } else if self.snapshot.cached {
                            "Local cache".into()
                        } else {
                            "Not reported".into()
                        },
                        if self.snapshot.live {
                            Tone::Good
                        } else {
                            Tone::Neutral
                        },
                        palette,
                    )),
            )
            .child(
                panel(palette)
                    .row_start(if compact { 3 } else { 2 })
                    .col_start(1)
                    .col_span_full()
                    .child(section_heading(
                        IconName::Inspector,
                        "Collector diagnostics",
                        &diagnostics_detail,
                        palette,
                    ))
                    .when(!has_diagnostics, |view| {
                        view.child(
                            div()
                                .min_h(px(150.))
                                .flex()
                                .flex_col()
                                .items_center()
                                .justify_center()
                                .text_color(palette.muted)
                                .child(Icon::new(IconName::Info))
                                .child(
                                    div()
                                        .pt_3()
                                        .text_size(px(11.))
                                        .child("Run a sync to populate endpoint diagnostics"),
                                ),
                        )
                    })
                    .when(has_diagnostics, |view| {
                        view.child(
                            div()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .pb_2()
                                        .grid()
                                        .grid_cols(4)
                                        .text_size(px(9.))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(palette.subtle)
                                        .child("Endpoint")
                                        .child("Status")
                                        .child("Duration")
                                        .child("Result"),
                                )
                                .children(self.snapshot.collector.endpoints.iter().take(30).map(
                                    |endpoint| {
                                        div()
                                            .py_2()
                                            .border_b_1()
                                            .border_color(palette.border)
                                            .grid()
                                            .grid_cols(4)
                                            .text_size(px(11.))
                                            .child(format!("{} {}", endpoint.method, endpoint.path))
                                            .child(
                                                endpoint
                                                    .status
                                                    .map(|status| status.to_string())
                                                    .unwrap_or_else(|| "—".into()),
                                            )
                                            .child(format!("{:.0} ms", endpoint.duration_ms))
                                            .child(
                                                div()
                                                    .font_family(FONT_MONO)
                                                    .text_size(px(9.))
                                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                                    .text_color(if endpoint.ok {
                                                        palette.good
                                                    } else if endpoint.optional {
                                                        palette.warn
                                                    } else {
                                                        palette.bad
                                                    })
                                                    .child(if endpoint.ok {
                                                        "OK"
                                                    } else if endpoint.optional {
                                                        "OPTIONAL"
                                                    } else {
                                                        "FAILED"
                                                    }),
                                            )
                                    },
                                )),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_inspector_overview(
        &self,
        resource: &ResourceRow,
        key: &str,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let preference = self
            .worker_preferences
            .get(key)
            .copied()
            .unwrap_or_default();
        div()
            .flex()
            .flex_col()
            .child(metric_rows(
                [
                    ("Primary", resource.primary_metric.clone()),
                    ("Secondary", resource.secondary_metric.clone()),
                    (
                        "Updated",
                        resource
                            .updated_at
                            .clone()
                            .unwrap_or_else(|| "Unknown".into()),
                    ),
                    (
                        "Bindings",
                        resource.bindings.as_ref().map_or(0, Vec::len).to_string(),
                    ),
                ],
                palette,
            ))
            .when(resource.kind == "worker", |view| {
                view.child(
                    div()
                        .pt_5()
                        .child(
                            div()
                                .pb_2()
                                .text_size(px(10.))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(palette.subtle)
                                .child("Audit handling"),
                        )
                        .child(
                            div().flex().gap_2().children(
                                [
                                    (WorkerAuditPreference::Normal, "Normal"),
                                    (WorkerAuditPreference::Critical, "Critical"),
                                    (WorkerAuditPreference::Ignore, "Ignore"),
                                ]
                                .into_iter()
                                .map(|(value, label)| {
                                    Button::new(SharedString::from(format!("preference-{label}")))
                                        .label(label)
                                        .when(preference == value, |button| {
                                            button.bg(palette.selected).text_color(palette.accent)
                                        })
                                        .when(preference != value, |button| button.ghost())
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.set_worker_preference(value, cx)
                                        }))
                                }),
                            ),
                        ),
                )
            })
            .into_any_element()
    }

    fn render_inspector_bindings(&self, resource: &ResourceRow, palette: Palette) -> AnyElement {
        let bindings = resource.bindings.clone().unwrap_or_default();
        if bindings.is_empty() {
            return inspector_empty_state(
                IconName::Frame,
                "No bindings discovered",
                "This resource has no binding metadata in the latest snapshot.",
                palette,
            )
            .into_any_element();
        }

        div()
            .flex()
            .flex_col()
            .gap_2()
            .children(bindings.into_iter().enumerate().map(|(index, binding)| {
                let binding_type = binding
                    .binding_type
                    .or(binding.resource_kind)
                    .unwrap_or_else(|| "binding".into());
                let target = binding
                    .resource_name
                    .or(binding.resource_id)
                    .unwrap_or_else(|| "No linked resource metadata".into());
                div()
                    .id(("resource-binding", index))
                    .p_3()
                    .rounded(px(8.))
                    .bg(palette.surface)
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .gap_3()
                            .child(
                                div()
                                    .min_w_0()
                                    .font_family(FONT_MONO)
                                    .text_size(px(12.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(binding.name),
                            )
                            .child(
                                div()
                                    .text_size(px(9.))
                                    .text_color(palette.accent)
                                    .child(binding_type.to_uppercase()),
                            ),
                    )
                    .child(
                        div()
                            .pt_2()
                            .text_size(px(10.))
                            .text_color(palette.muted)
                            .child(target),
                    )
            }))
            .into_any_element()
    }

    fn render_inspector_observability(
        &self,
        resource: &ResourceRow,
        palette: Palette,
    ) -> AnyElement {
        let Some(observability) = resource.observability.clone() else {
            return inspector_empty_state(
                IconName::Eye,
                "No observability metadata",
                "Cedar did not receive logs, traces, or destination configuration for this resource.",
                palette,
            )
            .into_any_element();
        };

        let destinations = observability.destinations.clone();
        div()
            .flex()
            .flex_col()
            .child(metric_rows(
                [
                    ("Observability", optional_flag(observability.enabled)),
                    ("Logs", optional_flag(observability.logs_enabled)),
                    ("Traces", optional_flag(observability.traces_enabled)),
                    (
                        "Invocation logs",
                        optional_flag(observability.invocation_logs),
                    ),
                    ("Logpush", optional_flag(observability.logpush)),
                    (
                        "Head sampling",
                        observability
                            .head_sampling_rate
                            .map(|rate| format!("{:.1}%", rate * 100.0))
                            .unwrap_or_else(|| "Unknown".into()),
                    ),
                ],
                palette,
            ))
            .child(
                div()
                    .pt_5()
                    .pb_2()
                    .text_size(px(10.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(palette.subtle)
                    .child(format!("Destinations · {}", destinations.len())),
            )
            .when(destinations.is_empty(), |view| {
                view.child(
                    div()
                        .p_3()
                        .rounded(px(10.))
                        .bg(palette.surface)
                        .text_size(px(11.))
                        .text_color(palette.muted)
                        .child("No destinations reported"),
                )
            })
            .when(!destinations.is_empty(), |view| {
                view.children(
                    destinations
                        .into_iter()
                        .enumerate()
                        .map(|(index, destination)| {
                            div()
                                .id(("observability-destination", index))
                                .py_2()
                                .border_b_1()
                                .border_color(palette.border)
                                .font_family(FONT_MONO)
                                .text_size(px(10.))
                                .child(destination)
                        }),
                )
            })
            .into_any_element()
    }

    fn render_inspector_audit(
        &self,
        resource: &ResourceRow,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = resource.name.to_lowercase();
        let id = resource.id.to_lowercase();
        let findings = build_audit_findings(&self.snapshot, &self.worker_preferences)
            .into_iter()
            .filter(|finding| {
                finding.evidence.iter().any(|evidence| {
                    let evidence = evidence.to_lowercase();
                    evidence.contains(&name) || evidence.contains(&id)
                })
            })
            .collect::<Vec<_>>();
        let tone = match resource.status.as_str() {
            "healthy" | "ok" => Tone::Good,
            "warning" | "quiet" => Tone::Warn,
            "error" => Tone::Bad,
            _ => Tone::Neutral,
        };

        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .p_4()
                    .rounded(px(8.))
                    .bg(palette.surface)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(status_dot(tone))
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child("Latest snapshot assessment"),
                            ),
                    )
                    .child(
                        div()
                            .pt_2()
                            .text_size(px(11.))
                            .line_height(px(17.))
                            .text_color(palette.muted)
                            .child(resource_assessment(resource)),
                    ),
            )
            .when(findings.is_empty(), |view| {
                view.child(
                    div()
                        .pt_2()
                        .text_size(px(11.))
                        .text_color(palette.muted)
                        .child("No account-level finding names this resource directly."),
                )
            })
            .when(!findings.is_empty(), |view| {
                view.child(
                    div()
                        .pt_2()
                        .pb_1()
                        .text_size(px(10.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(palette.subtle)
                        .child("Related findings"),
                )
                .children(
                    findings.into_iter().enumerate().map(|(index, finding)| {
                        finding_card(finding, index + 1, false, palette, cx)
                    }),
                )
            })
            .into_any_element()
    }

    fn render_finding_drawer(&self, finding: AuditFinding, cx: &mut Context<Self>) -> AnyElement {
        let palette = self.palette();
        let tone = finding.tone;
        let evidence = finding
            .evidence
            .into_iter()
            .map(|evidence| {
                let resource_key = evidence_resource_key(&evidence, &self.snapshot.resources);
                (evidence, resource_key)
            })
            .collect::<Vec<_>>();
        let evidence_count = evidence.len();
        let action = finding.action;
        let section = finding.section;
        let section_cta = section.map(|section| format!("Open {}", section_label(section)));
        let generated_at = if self.snapshot.generated_at.is_empty() {
            "Not synced".into()
        } else {
            self.snapshot.generated_at.clone()
        };

        div()
            .min_w_0()
            .size_full()
            .border_l_1()
            .border_color(palette.border)
            .bg(palette.panel)
            .shadow(palette.panel_shadow())
            .flex()
            .flex_col()
            .child(
                div()
                    .px_5()
                    .pt_4()
                    .pb_3()
                    .flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(9.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(palette.subtle)
                            .child("FINDING INSPECTOR"),
                    )
                    .child(
                        Button::new("close-finding-drawer")
                            .ghost()
                            .icon(IconName::Close)
                            .tooltip("Close inspector")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.selected_finding = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .px_5()
                    .pb_4()
                    .border_b_1()
                    .border_color(palette.border)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(status_dot(tone))
                            .child(
                                div()
                                    .font_family(FONT_MONO)
                                    .text_size(px(10.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(tone_color(tone, palette))
                                    .child(tone.label().to_uppercase()),
                            ),
                    )
                    .child(
                        div()
                            .pt_3()
                            .text_size(px(20.))
                            .line_height(px(25.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(finding.title),
                    )
                    .child(
                        div()
                            .pt_3()
                            .text_size(px(11.))
                            .line_height(px(18.))
                            .text_color(palette.muted)
                            .child(finding.detail),
                    ),
            )
            .child(
                div()
                    .min_h_0()
                    .flex_grow()
                    .px_5()
                    .pt_4()
                    .pb_6()
                    .when_some(action, |content, action| {
                        content.child(
                            div()
                                .mb_5()
                                .p_4()
                                .rounded(px(8.))
                                .border_1()
                                .border_color(palette.accent.opacity(0.4))
                                .bg(palette.raised)
                                .child(
                                    div()
                                        .text_size(px(10.))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(palette.accent)
                                        .child("RECOMMENDED ACTION"),
                                )
                                .child(
                                    div()
                                        .pt_3()
                                        .text_size(px(11.))
                                        .line_height(px(18.))
                                        .child(action),
                                ),
                        )
                    })
                    .child(
                        div()
                            .pb_2()
                            .flex()
                            .items_center()
                            .justify_between()
                            .text_size(px(10.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(palette.subtle)
                            .child("Evidence")
                            .child(evidence_count.to_string()),
                    )
                    .when(evidence.is_empty(), |content| {
                        content.child(
                            div()
                                .p_4()
                                .rounded(px(8.))
                                .bg(palette.surface)
                                .text_size(px(11.))
                                .line_height(px(17.))
                                .text_color(palette.muted)
                                .child("No resource-level evidence was attached to this finding."),
                        )
                    })
                    .when(!evidence.is_empty(), |content| {
                        content.children(evidence.into_iter().enumerate().map(
                            |(index, (evidence, resource_key))| {
                                div()
                                    .id(("finding-evidence", index))
                                    .py_3()
                                    .px_2()
                                    .rounded(px(7.))
                                    .border_b_1()
                                    .border_color(palette.border)
                                    .flex()
                                    .items_start()
                                    .gap_3()
                                    .child(
                                        div()
                                            .w(px(24.))
                                            .flex_none()
                                            .font_family(FONT_MONO)
                                            .text_size(px(10.))
                                            .text_color(palette.accent)
                                            .child(format!("{:02}", index + 1)),
                                    )
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex_grow()
                                            .text_size(px(10.))
                                            .line_height(px(16.))
                                            .text_color(palette.muted)
                                            .child(evidence),
                                    )
                                    .when_some(resource_key, |row, key| {
                                        row.child(
                                            Button::new(SharedString::from(format!(
                                                "open-finding-evidence-{index}"
                                            )))
                                            .ghost()
                                            .compact()
                                            .icon(IconName::ArrowRight)
                                            .label("Open")
                                            .tooltip("Open linked resource")
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.open_investigation_resource(
                                                    key.clone(),
                                                    window,
                                                    cx,
                                                )
                                            })),
                                        )
                                    })
                            },
                        ))
                    })
                    .child(
                        div()
                            .pt_5()
                            .child(section_heading(
                                IconName::Frame,
                                "Snapshot context",
                                "Audit boundary for this finding",
                                palette,
                            ))
                            .child(metric_rows(
                                [
                                    ("Range", self.range.to_string()),
                                    ("Generated", generated_at),
                                    ("Evidence", evidence_count.to_string()),
                                ],
                                palette,
                            )),
                    )
                    .overflow_y_scrollbar(),
            )
            .when_some(section.zip(section_cta), |drawer, (section, label)| {
                drawer.child(
                    div().p_4().border_t_1().border_color(palette.border).child(
                        Button::new("open-finding-section")
                            .primary()
                            .label(label)
                            .w_full()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.active_section = section;
                                this.focus_investigation(window, cx);
                            })),
                    ),
                )
            })
            .into_any_element()
    }

    fn render_detail_drawer(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if let Some(finding) = self.selected_finding.clone() {
            return Some(self.render_finding_drawer(finding, cx));
        }
        let key = self.selected_resource.as_deref()?;
        let resource = self
            .snapshot
            .resources
            .iter()
            .find(|resource| resource_key(resource) == key)?
            .clone();
        let palette = self.palette();
        let content = match self.inspector_tab {
            InspectorTab::Overview => self.render_inspector_overview(&resource, key, palette, cx),
            InspectorTab::Bindings => self.render_inspector_bindings(&resource, palette),
            InspectorTab::Observability => self.render_inspector_observability(&resource, palette),
            InspectorTab::Audit => self.render_inspector_audit(&resource, palette, cx),
        };
        let can_go_back = self.inspector_history_target(-1).is_some();
        let can_go_forward = self.inspector_history_target(1).is_some();
        let resource_id = resource.id.clone();
        let drawer_animation_id = SharedString::from(format!("resource-inspector-{key}"));
        let content_animation_id = SharedString::from(format!(
            "resource-inspector-{key}-tab-{}",
            self.inspector_tab.index()
        ));

        Some(
            div()
                .min_w_0()
                .size_full()
                .border_l_1()
                .border_color(palette.border)
                .bg(palette.panel)
                .shadow(palette.panel_shadow())
                .flex()
                .flex_col()
                .child(
                    div()
                        .px_5()
                        .pt_4()
                        .flex()
                        .justify_between()
                        .items_center()
                        .child(
                            div()
                                .text_size(px(11.))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(palette.subtle)
                                .child("Inspector"),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(
                                    Button::new("inspector-back")
                                        .ghost()
                                        .icon(IconName::ArrowLeft)
                                        .tooltip("Previous resource · Alt Left")
                                        .disabled(!can_go_back)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.navigate_inspector_history(-1, cx);
                                        })),
                                )
                                .child(
                                    Button::new("inspector-forward")
                                        .ghost()
                                        .icon(IconName::ArrowRight)
                                        .tooltip("Next resource · Alt Right")
                                        .disabled(!can_go_forward)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.navigate_inspector_history(1, cx);
                                        })),
                                )
                                .child(
                                    Button::new("copy-resource-id")
                                        .ghost()
                                        .icon(IconName::Copy)
                                        .tooltip("Copy resource ID")
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                resource_id.clone(),
                                            ));
                                            this.show_notice(
                                                "Resource ID copied",
                                                false,
                                                window,
                                                cx,
                                            );
                                        })),
                                )
                                .child(
                                    div()
                                        .mx_1()
                                        .h(px(16.))
                                        .border_l_1()
                                        .border_color(palette.border),
                                )
                                .child(
                                    Button::new("close-drawer")
                                        .ghost()
                                        .icon(IconName::Close)
                                        .tooltip("Close inspector")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.selected_resource = None;
                                            this.resource_table
                                                .update(cx, |table, cx| table.clear_selection(cx));
                                            cx.notify();
                                        })),
                                ),
                        ),
                )
                .child(
                    div()
                        .px_5()
                        .pt_2()
                        .pb_3()
                        .child(
                            div()
                                .text_size(px(20.))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(resource.name.clone()),
                        )
                        .child(
                            div()
                                .pt_2()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(status_pill(&resource.status, palette))
                                .child(
                                    div()
                                        .text_size(px(10.))
                                        .text_color(palette.muted)
                                        .child(resource.kind.to_uppercase()),
                                ),
                        )
                        .child(
                            div()
                                .pt_3()
                                .font_family(FONT_MONO)
                                .text_size(px(9.))
                                .text_color(palette.subtle)
                                .text_ellipsis()
                                .child(resource.id.clone()),
                        ),
                )
                .child(
                    div().px_5().child(
                        TabBar::new("resource-inspector-tabs")
                            .underline()
                            .selected_index(self.inspector_tab.index())
                            .children(["Overview", "Bindings", "Observability", "Audit"])
                            .on_click(cx.listener(|this, index, _, cx| {
                                this.inspector_tab = InspectorTab::from_index(*index);
                                cx.notify();
                            })),
                    ),
                )
                .child(
                    div()
                        .min_h_0()
                        .flex_grow()
                        .px_5()
                        .pt_4()
                        .pb_6()
                        .child(
                            div().child(content).with_animation(
                                content_animation_id,
                                Animation::new(motion_duration(self.reduced_motion, 140))
                                    .with_easing(ease_out_quint()),
                                |view, delta| {
                                    view.relative().top(px(6.) - delta * px(6.)).opacity(delta)
                                },
                            ),
                        )
                        .overflow_y_scrollbar(),
                )
                .with_animation(
                    drawer_animation_id,
                    Animation::new(motion_duration(self.reduced_motion, 160))
                        .with_easing(ease_out_quint()),
                    |drawer, delta| {
                        drawer
                            .relative()
                            .right(px(-10.) + delta * px(10.))
                            .opacity(delta)
                    },
                )
                .into_any_element(),
        )
    }
}

impl Render for CedarApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.persist_workspace_if_changed(cx);
        let palette = self.palette();
        let available_content_width = f32::from(window.viewport_size().width)
            - if self.sidebar_collapsed { 56. } else { 216. }
            - if self.selected_resource.is_some() || self.selected_finding.is_some() {
                self.inspector_width
            } else {
                0.
            };
        let compact = available_content_width < 1100.;
        let wide = available_content_width >= 1360.;
        let topbar_compact = available_content_width < 1200.;
        let workspace_width = if compact {
            available_content_width - 32.
        } else {
            (available_content_width - 48.).min(1440.)
        }
        .max(320.);
        let content = if self.syncing && self.snapshot.generated_at.is_empty() && self.connected() {
            self.render_loading_state()
        } else {
            match self.active_section {
                Section::Overview => self.render_overview(compact, cx),
                Section::Resources => self.render_resources(false, compact, workspace_width, cx),
                Section::Workers => self.render_resources(true, compact, workspace_width, cx),
                Section::Billing => self.render_billing(compact, wide),
                Section::Connection => self.render_connection(compact, wide, cx),
                Section::Settings => self.render_settings(compact, cx),
            }
        };
        let content = div().w_full().child(content).with_animation(
            SharedString::from(format!("snapshot-content-{}", self.snapshot.generated_at)),
            Animation::new(motion_duration(self.reduced_motion, 180)).with_easing(ease_out_quint()),
            |view, delta| view.opacity(delta),
        );
        let investigation_ribbon = self.render_investigation_ribbon(compact, cx);
        let main = div()
            .min_w_0()
            .h_full()
            .flex_grow()
            .flex()
            .flex_col()
            .child(self.render_topbar(topbar_compact, cx))
            .when_some(investigation_ribbon, |main, ribbon| main.child(ribbon))
            .child(
                div()
                    .id("main-scroll")
                    .min_h_0()
                    .flex_grow()
                    .flex()
                    .flex_col()
                    .when(compact, |scroll| scroll.px_4().pt_4())
                    .when(!compact, |scroll| scroll.px_6().pt_5())
                    .pb_6()
                    .child(
                        div()
                            .min_h_0()
                            .flex_grow()
                            .max_w(px(1440.))
                            .w_full()
                            .mx_auto()
                            .flex()
                            .flex_col()
                            .gap_4()
                            .when_some(self.error.clone(), |view, error| {
                                view.child(error_banner(error, palette, cx))
                            })
                            .child(content),
                    )
                    .overflow_y_scrollbar(),
            );

        let inspector_width = self.inspector_width;
        let inspector_owner = cx.entity().downgrade();
        let workspace = if let Some(drawer) = self.render_detail_drawer(cx) {
            h_resizable("cedar-workspace-inspector")
                .with_state(&self.inspector_split)
                .on_resize(move |state, _, cx| {
                    let Some(width) = state.read(cx).sizes().last().copied().map(f32::from) else {
                        return;
                    };
                    if let Some(owner) = inspector_owner.upgrade() {
                        owner.update(cx, |this, cx| {
                            this.resize_inspector(width, cx);
                        });
                    }
                })
                .child(
                    resizable_panel()
                        .size_range(px(520.)..Pixels::MAX)
                        .child(main),
                )
                .child(
                    resizable_panel()
                        .size(px(inspector_width))
                        .size_range(px(320.)..px(560.))
                        .child(drawer),
                )
                .into_any_element()
        } else {
            main.into_any_element()
        };
        let command_palette = self
            .command_palette_open
            .then(|| self.render_command_palette(cx));
        let shortcut_guide = self
            .shortcut_guide_open
            .then(|| self.render_shortcut_guide(window, cx));

        let body = div()
            .w_full()
            .min_h_0()
            .flex_grow()
            .flex()
            .relative()
            .bg(palette.background)
            .text_color(palette.foreground)
            .child(self.render_sidebar(cx))
            .child(div().min_w_0().h_full().flex_grow().child(workspace))
            .when_some(self.notice.clone(), |view, notice| {
                view.child(static_notice(notice, palette, cx))
            })
            .when_some(command_palette, |view, palette| view.child(palette))
            .when_some(shortcut_guide, |view, guide| view.child(guide));

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(palette.background)
            .text_color(palette.foreground)
            .track_focus(&self.investigation_focus)
            .on_action(cx.listener(Self::focus_resource_search))
            .on_action(cx.listener(Self::refresh_dashboard))
            .on_action(cx.listener(Self::close_resource_inspector))
            .on_action(cx.listener(Self::toggle_command_palette))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::toggle_shortcut_guide))
            .on_action(cx.listener(Self::inspector_back))
            .on_action(cx.listener(Self::inspector_forward))
            .on_key_down(cx.listener(Self::investigation_key_down))
            .child(self.render_titlebar(window))
            .child(body)
    }
}

fn section_name(section: Section) -> &'static str {
    match section {
        Section::Overview => "overview",
        Section::Resources => "resources",
        Section::Workers => "workers",
        Section::Billing => "billing",
        Section::Connection => "connection",
        Section::Settings => "settings",
    }
}

fn section_from_name(name: &str) -> Section {
    match name {
        "resources" => Section::Resources,
        "workers" => Section::Workers,
        "billing" => Section::Billing,
        "connection" => Section::Connection,
        "settings" => Section::Settings,
        _ => Section::Overview,
    }
}

fn inspector_tab_name(tab: InspectorTab) -> &'static str {
    match tab {
        InspectorTab::Overview => "overview",
        InspectorTab::Bindings => "bindings",
        InspectorTab::Observability => "observability",
        InspectorTab::Audit => "audit",
    }
}

fn inspector_tab_from_name(name: &str) -> InspectorTab {
    match name {
        "bindings" => InspectorTab::Bindings,
        "observability" => InspectorTab::Observability,
        "audit" => InspectorTab::Audit,
        _ => InspectorTab::Overview,
    }
}

fn visual_qa_snapshot() -> DashboardSnapshot {
    serde_json::from_str(include_str!("../tests/fixtures/visual_qa_snapshot.json"))
        .expect("visual QA fixture must remain valid")
}

fn empty_snapshot() -> DashboardSnapshot {
    DashboardSnapshot {
        range: "24h".into(),
        health: vec![crate::backend::ServiceHealth {
            id: "connection".into(),
            service: "Connection".into(),
            status: "unknown".into(),
            label: "Setup required".into(),
            detail: "Connect a Cloudflare API token to load live inventory and usage.".into(),
        }],
        ..DashboardSnapshot::default()
    }
}

fn format_updated_at(value: &str) -> String {
    let Ok(timestamp) = chrono::DateTime::parse_from_rfc3339(value) else {
        return "UPDATED".into();
    };
    let local = timestamp.with_timezone(&chrono::Local);
    if local.date_naive() == chrono::Local::now().date_naive() {
        format!("UPDATED {}", local.format("%H:%M"))
    } else {
        format!("UPDATED {}", local.format("%b %d · %H:%M")).to_uppercase()
    }
}

fn resource_key(resource: &ResourceRow) -> String {
    format!("{}-{}", resource.kind, resource.id)
}

fn section_for_resource(resource: &ResourceRow) -> Section {
    if resource.kind == "worker" {
        Section::Workers
    } else {
        Section::Resources
    }
}

fn evidence_resource_key(evidence: &str, resources: &[ResourceRow]) -> Option<String> {
    let normalized = evidence.trim().to_lowercase();
    resources
        .iter()
        .filter(|resource| {
            let name = resource.name.trim().to_lowercase();
            !name.is_empty()
                && (normalized == name
                    || normalized.starts_with(&format!("{name} ("))
                    || normalized == resource_key(resource).to_lowercase())
        })
        .max_by_key(|resource| resource.name.len())
        .map(resource_key)
}

fn finding_resource_keys(finding: &AuditFinding, resources: &[ResourceRow]) -> Vec<String> {
    let mut seen = HashSet::new();
    finding
        .evidence
        .iter()
        .filter_map(|evidence| evidence_resource_key(evidence, resources))
        .filter(|key| seen.insert(key.clone()))
        .collect()
}

fn resource_kind_label(kind: &str) -> &str {
    match kind {
        "worker" => "Workers",
        "page" => "Pages",
        "d1" => "D1 databases",
        "r2" => "R2 buckets",
        "kv" => "KV namespaces",
        _ => "Other",
    }
}

fn section_label(section: Section) -> &'static str {
    match section {
        Section::Overview => "Audit",
        Section::Resources => "Resources",
        Section::Workers => "Workers",
        Section::Billing => "Cost",
        Section::Connection => "Connection",
        Section::Settings => "Settings",
    }
}

fn resource_kind_icon(kind: &str) -> IconName {
    match kind {
        "worker" => IconName::SquareTerminal,
        "page" => IconName::Globe,
        _ => IconName::Frame,
    }
}

fn resource_status_tone(status: &str) -> Tone {
    match status {
        "healthy" | "ok" => Tone::Good,
        "warning" | "quiet" | "degraded" => Tone::Warn,
        "error" => Tone::Bad,
        _ => Tone::Neutral,
    }
}

fn fit_topology_view(layout: &TopologyLayout, width: f32, height: f32) -> (f32, f32, f32) {
    let Some(first) = layout.nodes.first() else {
        return (1., 18., 12.);
    };
    let min_x = layout
        .nodes
        .iter()
        .map(|node| node.x)
        .fold(first.x, f32::min);
    let max_x = layout
        .nodes
        .iter()
        .map(|node| node.x)
        .fold(first.x, f32::max);
    let max_y = layout
        .nodes
        .iter()
        .map(|node| node.y)
        .fold(first.y, f32::max);
    let horizontal_span = max_x - min_x;
    let vertical_span = (max_y - 18.).max(1.);
    let horizontal_scale = if horizontal_span > 0. {
        ((width - 36. - TOPOLOGY_NODE_WIDTH) / horizontal_span).max(0.)
    } else {
        1.
    };
    let vertical_scale = ((height - 30. - TOPOLOGY_NODE_HEIGHT) / vertical_span).max(0.);
    let scale = horizontal_scale.min(vertical_scale).clamp(0.72, 1.);
    let rendered_width = horizontal_span * scale + TOPOLOGY_NODE_WIDTH;
    let rendered_height = vertical_span * scale + TOPOLOGY_NODE_HEIGHT;
    let offset_x = (width - rendered_width) / 2. - min_x * scale;
    let offset_y = (height - rendered_height) / 2. - 18. * scale;
    (scale, offset_x, offset_y)
}

fn build_topology_layout(resources: &[ResourceRow]) -> TopologyLayout {
    if resources.is_empty() {
        return TopologyLayout::default();
    }

    let mut present_groups = [false; 3];
    for resource in resources {
        present_groups[topology_group(&resource.kind)] = true;
    }
    let mut group_columns = [0usize; 3];
    let mut lanes = Vec::new();
    for (group, present) in present_groups.into_iter().enumerate() {
        if !present {
            continue;
        }
        let column = lanes.len();
        group_columns[group] = column;
        lanes.push(TopologyLane {
            label: match group {
                0 => "COMPUTE",
                1 => "DATA & STATE",
                _ => "STORAGE & EDGE",
            },
            x: 56. + column as f32 * 340.,
        });
    }
    let mut ordered = resources.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| {
        topology_group(&left.kind)
            .cmp(&topology_group(&right.kind))
            .then_with(|| topology_kind_order(&left.kind).cmp(&topology_kind_order(&right.kind)))
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.id.cmp(&right.id))
    });

    let mut column_rows = vec![0usize; lanes.len()];
    let nodes = ordered
        .into_iter()
        .map(|resource| {
            let column = group_columns[topology_group(&resource.kind)];
            let row = column_rows[column];
            column_rows[column] += 1;
            TopologyNode {
                key: resource_key(resource),
                name: resource.name.clone(),
                kind: resource.kind.clone(),
                status: resource.status.clone(),
                x: 56. + column as f32 * 340.,
                y: 58. + row as f32 * 84.,
            }
        })
        .collect::<Vec<_>>();

    let mut by_key = HashMap::with_capacity(nodes.len());
    let mut by_id = HashMap::with_capacity(nodes.len());
    let mut by_name = HashMap::with_capacity(nodes.len());
    for (index, node) in nodes.iter().enumerate() {
        by_key.insert(node.key.to_lowercase(), index);
        by_id
            .entry(node.key[node.kind.len() + 1..].to_lowercase())
            .or_insert(index);
        by_name.entry(node.name.to_lowercase()).or_insert(index);
    }

    let source_by_key = resources
        .iter()
        .map(|resource| (resource_key(resource), resource))
        .collect::<HashMap<_, _>>();
    let mut edges = Vec::new();
    let mut edge_set = HashSet::new();
    for (from, node) in nodes.iter().enumerate() {
        let Some(resource) = source_by_key.get(&node.key) else {
            continue;
        };
        for binding in resource.bindings.iter().flatten() {
            let target = binding
                .resource_kind
                .as_deref()
                .zip(binding.resource_id.as_deref())
                .and_then(|(kind, id)| by_key.get(&format!("{kind}-{id}").to_lowercase()))
                .or_else(|| {
                    binding
                        .resource_id
                        .as_deref()
                        .and_then(|id| by_id.get(&id.to_lowercase()))
                })
                .or_else(|| {
                    binding.resource_name.as_deref().and_then(|name| {
                        binding
                            .resource_kind
                            .as_deref()
                            .and_then(|kind| by_key.get(&format!("{kind}-{name}").to_lowercase()))
                            .or_else(|| by_name.get(&name.to_lowercase()))
                    })
                });
            if let Some(&to) = target
                && from != to
                && edge_set.insert((from, to))
            {
                edges.push(TopologyEdge { from, to });
            }
        }
    }

    TopologyLayout {
        nodes,
        edges,
        lanes,
    }
}

fn topology_group(kind: &str) -> usize {
    match kind {
        "worker" | "page" => 0,
        "d1" | "kv" => 1,
        _ => 2,
    }
}

fn topology_kind_order(kind: &str) -> usize {
    match kind {
        "worker" => 0,
        "page" => 1,
        "d1" => 2,
        "kv" => 3,
        "r2" => 4,
        _ => 5,
    }
}

fn resource_table_fingerprint(
    resources: &[ResourceRow],
    workers_only: bool,
    status_filter: &str,
    dark: bool,
) -> String {
    let mut fingerprint = format!("{workers_only}|{status_filter}|{dark}");
    for resource in resources {
        fingerprint.push('\u{1f}');
        fingerprint.push_str(&resource.id);
        fingerprint.push('\u{1f}');
        fingerprint.push_str(&resource.name);
        fingerprint.push('\u{1f}');
        fingerprint.push_str(&resource.kind);
        fingerprint.push('\u{1f}');
        fingerprint.push_str(&resource.status);
        fingerprint.push('\u{1f}');
        fingerprint.push_str(&resource.primary_metric);
        fingerprint.push('\u{1f}');
        fingerprint.push_str(&resource.secondary_metric);
    }
    fingerprint
}

fn optional_flag(value: Option<bool>) -> String {
    match value {
        Some(true) => "Enabled".into(),
        Some(false) => "Disabled".into(),
        None => "Unknown".into(),
    }
}

fn resource_assessment(resource: &ResourceRow) -> String {
    match resource.status.as_str() {
        "healthy" | "ok" => format!(
            "{} is healthy in the latest snapshot. No direct resource-level issue was detected.",
            resource.name
        ),
        "quiet" => format!(
            "{} had no request signal in the selected range. Confirm that quiet traffic is expected.",
            resource.name
        ),
        "warning" => format!(
            "{} needs attention in the latest snapshot. Review its metrics and observability coverage.",
            resource.name
        ),
        "error" => format!(
            "{} reported an error signal. Treat this resource as an active investigation target.",
            resource.name
        ),
        _ => format!(
            "{} has no conclusive health signal in the latest snapshot.",
            resource.name
        ),
    }
}

fn shortcut_badge(shortcut: &str, palette: Palette) -> gpui::Div {
    div()
        .px_2()
        .py_1()
        .rounded(px(6.))
        .border_1()
        .border_color(palette.border)
        .bg(palette.surface)
        .font_family(FONT_MONO)
        .text_size(px(9.))
        .text_color(palette.muted)
        .child(shortcut.to_string())
}

fn shortcut_group(
    icon: IconName,
    title: &str,
    shortcuts: &[(&str, &str)],
    palette: Palette,
) -> gpui::Div {
    div()
        .h(px(42. + shortcuts.len() as f32 * 38.))
        .flex_none()
        .rounded(px(12.))
        .border_1()
        .border_color(palette.border)
        .bg(palette.panel)
        .overflow_hidden()
        .child(
            div()
                .h(px(42.))
                .px_3()
                .border_b_1()
                .border_color(palette.border)
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(12.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(
                    div()
                        .size(px(24.))
                        .rounded(px(7.))
                        .bg(palette.surface)
                        .text_color(palette.accent)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(Icon::new(icon).size_3()),
                )
                .child(title.to_string()),
        )
        .children(shortcuts.iter().enumerate().map(|(index, (label, key))| {
            div()
                .h(px(38.))
                .px_3()
                .when(index > 0, |row| {
                    row.border_t_1().border_color(palette.border.opacity(0.7))
                })
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .text_size(px(11.))
                .text_color(palette.muted)
                .child((*label).to_string())
                .child(shortcut_badge(key, palette))
        }))
}

fn inspector_empty_state(icon: IconName, title: &str, detail: &str, palette: Palette) -> gpui::Div {
    div()
        .min_h(px(250.))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .text_center()
        .child(
            div()
                .size(px(42.))
                .rounded(px(12.))
                .bg(palette.surface)
                .text_color(palette.accent)
                .flex()
                .items_center()
                .justify_center()
                .child(Icon::new(icon)),
        )
        .child(
            div()
                .pt_4()
                .text_size(px(13.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(title.to_string()),
        )
        .child(
            div()
                .pt_2()
                .max_w(px(290.))
                .text_size(px(11.))
                .line_height(px(17.))
                .text_color(palette.muted)
                .child(detail.to_string()),
        )
}

fn cedar_mark(size: f32, palette: Palette, dark: bool) -> AnyElement {
    div()
        .flex_none()
        .size(px(size))
        .when(!dark, |mark| {
            mark.p(px(1.))
                .rounded(px(size * 0.16))
                .bg(palette.foreground)
        })
        .child(img("cedar/app-icon.png").size_full())
        .into_any_element()
}

fn panel(palette: Palette) -> gpui::Div {
    div()
        .p_5()
        .rounded(px(11.))
        .border_1()
        .border_color(palette.border)
        .bg(palette.panel)
}

fn section_heading(icon: IconName, title: &str, description: &str, palette: Palette) -> gpui::Div {
    div()
        .pb_4()
        .flex()
        .items_start()
        .gap_3()
        .child(
            div()
                .size(px(30.))
                .flex_none()
                .rounded(px(9.))
                .bg(palette.accent_soft)
                .text_color(palette.accent)
                .flex()
                .items_center()
                .justify_center()
                .child(Icon::new(icon).size_3()),
        )
        .child(
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_size(px(15.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(title.to_string()),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(palette.muted)
                        .child(description.to_string()),
                ),
        )
}

fn metrics_strip<const N: usize>(stats: [(&str, String, &str); N], palette: Palette) -> gpui::Div {
    div()
        .rounded(px(11.))
        .border_1()
        .border_color(palette.border)
        .bg(palette.panel)
        .flex()
        .children(
            stats
                .into_iter()
                .enumerate()
                .map(|(index, (label, value, detail))| {
                    div()
                        .min_w_0()
                        .flex_1()
                        .px_5()
                        .pt_4()
                        .pb_5()
                        .when(index > 0, |item| {
                            item.border_l_1().border_color(palette.border)
                        })
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .text_size(px(10.))
                                .text_color(palette.muted)
                                .child(label.to_string())
                                .child(
                                    div()
                                        .font_family(FONT_MONO)
                                        .text_size(px(10.))
                                        .text_color(palette.subtle)
                                        .child(format!("{:02}", index + 1)),
                                ),
                        )
                        .child(
                            div()
                                .pt_2()
                                .font_family(FONT_MONO)
                                .text_size(px(22.))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(value),
                        )
                        .child(
                            div()
                                .pt_1()
                                .text_size(px(10.))
                                .text_color(palette.subtle)
                                .child(detail.to_string()),
                        )
                }),
        )
}

fn stat_card(
    label: &str,
    value: impl Into<SharedString>,
    detail: &str,
    palette: Palette,
) -> gpui::Div {
    div()
        .min_w_0()
        .flex_1()
        .p_4()
        .rounded(px(8.))
        .bg(palette.surface)
        .child(
            div()
                .text_size(px(9.))
                .text_color(palette.subtle)
                .child(label.to_string()),
        )
        .child(
            div()
                .pt_2()
                .font_family(FONT_MONO)
                .text_size(px(21.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(value.into()),
        )
        .child(
            div()
                .pt_1()
                .text_size(px(10.))
                .text_color(palette.muted)
                .child(detail.to_string()),
        )
}

fn usage_trend_summary(points: &[u32]) -> Option<SharedString> {
    if points.len() < 2 {
        return None;
    }
    let first = f64::from(*points.first()?);
    let latest = f64::from(*points.last()?);
    if (first - latest).abs() < f64::EPSILON {
        return Some("flat".into());
    }
    if first == 0.0 {
        return Some("new".into());
    }

    Some(format!("{:+.1}%", ((latest - first) / first) * 100.0).into())
}

fn usage_sparkline(
    points: &[u32],
    animation_id: SharedString,
    palette: Palette,
    reduced_motion: bool,
) -> AnyElement {
    if points.len() < 2 {
        return div()
            .mt_3()
            .h(px(40.))
            .border_t_1()
            .border_color(palette.border)
            .flex()
            .items_center()
            .font_family(FONT_MONO)
            .text_size(px(10.))
            .text_color(palette.subtle)
            .child("NO TIME SERIES")
            .into_any_element();
    }

    let points = Arc::new(points.to_vec());
    let chart = canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let width = f32::from(bounds.size.width).max(1.);
            let height = f32::from(bounds.size.height).max(1.);
            let min = *points.iter().min().unwrap_or(&0) as f32;
            let max = *points.iter().max().unwrap_or(&0) as f32;
            let range = (max - min).max(1.);
            let horizontal_padding = 1.;
            let vertical_padding = 3.;
            let chart_width = (width - horizontal_padding * 2.).max(1.);
            let chart_height = (height - vertical_padding * 2.).max(1.);
            let last_index = (points.len() - 1) as f32;
            let chart_points = points
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    let x = horizontal_padding + chart_width * index as f32 / last_index;
                    let normalized = (*value as f32 - min) / range;
                    let y = vertical_padding + chart_height * (1. - normalized);
                    point(bounds.origin.x + px(x), bounds.origin.y + px(y))
                })
                .collect::<Vec<_>>();

            let mut baseline = PathBuilder::stroke(px(0.5));
            let baseline_y = bounds.origin.y + px(height - vertical_padding);
            baseline.move_to(point(bounds.origin.x, baseline_y));
            baseline.line_to(point(bounds.origin.x + bounds.size.width, baseline_y));
            if let Ok(path) = baseline.build() {
                window.paint_path(path, palette.border.opacity(0.52));
            }

            let mut area_points = Vec::with_capacity(chart_points.len() + 2);
            area_points.push(point(chart_points[0].x, baseline_y));
            area_points.extend(chart_points.iter().copied());
            area_points.push(point(chart_points[chart_points.len() - 1].x, baseline_y));
            let mut area = PathBuilder::fill();
            area.add_polygon(&area_points, true);
            if let Ok(path) = area.build() {
                window.paint_path(path, palette.accent.opacity(0.07));
            }

            let mut line = PathBuilder::stroke(px(1.5));
            line.move_to(chart_points[0]);
            for chart_point in chart_points.iter().skip(1) {
                line.line_to(*chart_point);
            }
            if let Ok(path) = line.build() {
                window.paint_path(path, palette.accent.opacity(0.9));
            }
        },
    )
    .size_full();

    div()
        .mt_2()
        .h(px(42.))
        .child(chart)
        .with_animation(
            animation_id,
            Animation::new(motion_duration(reduced_motion, 180)).with_easing(ease_out_quint()),
            |view, delta| view.opacity(delta),
        )
        .into_any_element()
}

fn activity_stream(
    changes: &[SnapshotChange],
    generated_at: &str,
    palette: Palette,
    reduced_motion: bool,
) -> AnyElement {
    let items = changes
        .iter()
        .take(5)
        .enumerate()
        .map(|(index, change)| {
            let tone = tone_color(change.tone, palette);
            let icon = match change.tone {
                Tone::Good => IconName::CircleCheck,
                Tone::Warn => IconName::Info,
                Tone::Bad => IconName::TriangleAlert,
                Tone::Neutral => IconName::Redo2,
            };
            div()
                .id(("snapshot-activity", index))
                .p_3()
                .rounded(px(8.))
                .bg(palette.surface)
                .flex()
                .items_start()
                .gap_3()
                .child(
                    div()
                        .size(px(28.))
                        .flex_none()
                        .rounded(px(7.))
                        .bg(tone.opacity(0.12))
                        .text_color(tone)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(Icon::new(icon).size_3()),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex_grow()
                        .child(
                            div()
                                .text_size(px(11.))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(change.title.clone()),
                        )
                        .child(
                            div()
                                .pt_1()
                                .text_size(px(10.))
                                .line_height(px(15.))
                                .text_color(palette.muted)
                                .child(change.detail.clone()),
                        ),
                )
        })
        .collect::<Vec<_>>();
    let change_count = changes.len();
    let hidden_count = change_count.saturating_sub(items.len());

    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .px_3()
                .py_2()
                .rounded(px(7.))
                .border_1()
                .border_color(palette.border)
                .flex()
                .items_center()
                .gap_2()
                .child(
                    Icon::new(IconName::Calendar)
                        .size_3()
                        .text_color(palette.muted),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex_grow()
                        .font_family(FONT_MONO)
                        .text_size(px(10.))
                        .text_color(palette.subtle)
                        .text_ellipsis()
                        .child(format!("DETECTED {generated_at}")),
                )
                .child(
                    div()
                        .flex_none()
                        .font_family(FONT_MONO)
                        .text_size(px(10.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(palette.accent)
                        .child(format!("{change_count} CHANGES")),
                ),
        )
        .children(items)
        .when(hidden_count > 0, |stream| {
            stream.child(
                div()
                    .pt_1()
                    .text_size(px(10.))
                    .text_color(palette.subtle)
                    .child(format!("+{hidden_count} more changes in this snapshot")),
            )
        })
        .with_animation(
            SharedString::from(format!("snapshot-activity-{generated_at}")),
            Animation::new(motion_duration(reduced_motion, 180)).with_easing(ease_out_quint()),
            |view, delta| view.opacity(delta),
        )
        .into_any_element()
}

fn finding_card(
    finding: AuditFinding,
    rank: usize,
    highlighted: bool,
    palette: Palette,
    cx: &mut Context<CedarApp>,
) -> AnyElement {
    let tone = finding.tone;
    let evidence_count = finding.evidence.len();
    let detail_finding = finding.clone();
    let keyboard_finding = finding.clone();
    div()
        .id(("audit-finding", rank))
        .p_4()
        .rounded(px(8.))
        .border_1()
        .border_color(if highlighted {
            palette.accent.opacity(0.42)
        } else {
            palette.border
        })
        .bg(if highlighted {
            palette.raised
        } else {
            palette.surface
        })
        .child(
            div()
                .flex()
                .items_start()
                .gap_3()
                .child(
                    div()
                        .size(px(32.))
                        .flex_none()
                        .rounded(px(8.))
                        .bg(if highlighted {
                            palette.accent
                        } else {
                            palette.panel
                        })
                        .border_1()
                        .border_color(if highlighted {
                            palette.accent
                        } else {
                            palette.border
                        })
                        .flex()
                        .items_center()
                        .justify_center()
                        .font_family(FONT_MONO)
                        .text_size(px(9.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(if highlighted {
                            color(0xffffff)
                        } else {
                            palette.muted
                        })
                        .child(format!("{rank:02}")),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex_grow()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_3()
                                .child(
                                    div()
                                        .min_w_0()
                                        .text_size(if highlighted { px(15.) } else { px(13.) })
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child(finding.title),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .font_family(FONT_MONO)
                                        .text_size(px(10.))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(tone_color(tone, palette))
                                        .child(tone.label().to_uppercase()),
                                ),
                        )
                        .child(
                            div()
                                .pt_2()
                                .text_size(px(11.))
                                .line_height(px(17.))
                                .text_color(palette.muted)
                                .child(finding.detail),
                        )
                        .when_some(finding.action, |view, action| {
                            view.child(
                                div()
                                    .pt_3()
                                    .flex()
                                    .items_start()
                                    .gap_2()
                                    .child(
                                        Icon::new(IconName::ArrowRight)
                                            .size_3()
                                            .text_color(palette.accent),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.))
                                            .line_height(px(15.))
                                            .text_color(palette.foreground)
                                            .child(action),
                                    ),
                            )
                        })
                        .child(
                            div()
                                .pt_3()
                                .flex()
                                .items_center()
                                .justify_between()
                                .text_size(px(10.))
                                .text_color(palette.subtle)
                                .child(div().font_family(FONT_MONO).child(if evidence_count == 1 {
                                    "1 EVIDENCE ITEM".to_string()
                                } else {
                                    format!("{evidence_count} EVIDENCE ITEMS")
                                }))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .child(
                                            Button::new(SharedString::from(format!(
                                                "view-finding-{rank}"
                                            )))
                                            .ghost()
                                            .compact()
                                            .icon(IconName::Eye)
                                            .label("Details")
                                            .tooltip("Open finding details")
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.open_finding_details(
                                                    detail_finding.clone(),
                                                    window,
                                                    cx,
                                                );
                                            })),
                                        )
                                        .child(
                                            Button::new(SharedString::from(format!(
                                                "investigate-finding-{rank}"
                                            )))
                                            .ghost()
                                            .compact()
                                            .icon(IconName::ArrowRight)
                                            .label("Investigate")
                                            .tooltip("Start a focused investigation")
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.start_investigation(
                                                    keyboard_finding.clone(),
                                                    window,
                                                    cx,
                                                );
                                            })),
                                        ),
                                ),
                        ),
                ),
        )
        .into_any_element()
}

fn tone_color(tone: Tone, palette: Palette) -> gpui::Hsla {
    match tone {
        Tone::Good => palette.good,
        Tone::Warn => palette.warn,
        Tone::Bad => palette.bad,
        Tone::Neutral => palette.muted,
    }
}

fn motion_duration(reduced_motion: bool, millis: u64) -> Duration {
    Duration::from_millis(if reduced_motion { 1 } else { millis })
}

fn loading_block(
    width: Option<Pixels>,
    height: Pixels,
    secondary: bool,
    reduced_motion: bool,
    palette: Palette,
) -> AnyElement {
    if reduced_motion {
        return div()
            .when_some(width, |view, width| view.w(width))
            .when(width.is_none(), |view| view.w_full())
            .h(height)
            .rounded(px(4.))
            .bg(if secondary {
                palette.border.opacity(0.5)
            } else {
                palette.border
            })
            .into_any_element();
    }

    Skeleton::new()
        .when_some(width, |view, width| view.w(width))
        .when(width.is_none(), |view| view.w_full())
        .h(height)
        .when(secondary, Skeleton::secondary)
        .into_any_element()
}

fn status_dot(tone: Tone) -> gpui::Div {
    let color = match tone {
        Tone::Good => color(0x32b67a),
        Tone::Warn => color(0xf2a23a),
        Tone::Bad => color(0xe15858),
        Tone::Neutral => color(0x858585),
    };
    div().size(px(8.)).rounded_full().bg(color).flex_none()
}

fn overview_signal(
    icon: IconName,
    label: &str,
    value: String,
    detail: &str,
    palette: Palette,
) -> gpui::Div {
    div()
        .min_w(px(180.))
        .flex_1()
        .flex()
        .items_center()
        .gap_3()
        .child(
            div()
                .size(px(30.))
                .flex_none()
                .rounded(px(9.))
                .bg(palette.surface)
                .text_color(palette.accent)
                .flex()
                .items_center()
                .justify_center()
                .child(Icon::new(icon).size_3()),
        )
        .child(
            div()
                .min_w_0()
                .flex_grow()
                .child(
                    div()
                        .text_size(px(9.))
                        .text_color(palette.muted)
                        .child(label.to_string()),
                )
                .child(
                    div()
                        .pt_1()
                        .flex()
                        .items_baseline()
                        .gap_2()
                        .child(
                            div()
                                .font_family(FONT_MONO)
                                .text_size(px(14.))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(value),
                        )
                        .child(
                            div()
                                .text_size(px(9.))
                                .text_color(palette.subtle)
                                .child(detail.to_string()),
                        ),
                ),
        )
}

fn connection_step(
    number: &str,
    title: &str,
    detail: &str,
    complete: bool,
    active: bool,
    palette: Palette,
) -> gpui::Div {
    div()
        .min_w_0()
        .flex()
        .items_center()
        .gap_3()
        .child(
            div()
                .size(px(30.))
                .flex_none()
                .rounded(px(9.))
                .border_1()
                .border_color(if complete || active {
                    palette.accent
                } else {
                    palette.border
                })
                .bg(if complete {
                    palette.accent_soft
                } else {
                    palette.surface
                })
                .text_color(if complete || active {
                    palette.accent
                } else {
                    palette.subtle
                })
                .flex()
                .items_center()
                .justify_center()
                .font_family(FONT_MONO)
                .text_size(px(9.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .when(complete, |step| step.child(Icon::new(IconName::Check)))
                .when(!complete, |step| step.child(number.to_string())),
        )
        .child(
            div()
                .min_w_0()
                .flex_grow()
                .child(
                    div()
                        .text_size(px(11.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(if active || complete {
                            palette.foreground
                        } else {
                            palette.muted
                        })
                        .child(title.to_string()),
                )
                .child(
                    div()
                        .pt_1()
                        .text_size(px(9.))
                        .text_color(palette.subtle)
                        .child(detail.to_string()),
                ),
        )
}

fn connection_scope(
    title: &str,
    detail: &str,
    label: &str,
    required: bool,
    palette: Palette,
) -> gpui::Div {
    div()
        .py_4()
        .border_b_1()
        .border_color(palette.border)
        .flex()
        .items_start()
        .gap_3()
        .child(
            div()
                .size(px(28.))
                .flex_none()
                .rounded(px(8.))
                .bg(if required {
                    palette.accent_soft
                } else {
                    palette.surface
                })
                .text_color(if required {
                    palette.accent
                } else {
                    palette.muted
                })
                .flex()
                .items_center()
                .justify_center()
                .child(Icon::new(if required {
                    IconName::CircleCheck
                } else {
                    IconName::Eye
                })),
        )
        .child(
            div()
                .min_w_0()
                .flex_grow()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .child(
                            div()
                                .text_size(px(11.))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(title.to_string()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .font_family(FONT_MONO)
                                .text_size(px(10.))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(if required {
                                    palette.accent
                                } else {
                                    palette.subtle
                                })
                                .child(label.to_string()),
                        ),
                )
                .child(
                    div()
                        .pt_2()
                        .text_size(px(10.))
                        .line_height(px(15.))
                        .text_color(palette.muted)
                        .child(detail.to_string()),
                ),
        )
}

fn connection_trust_item(icon: IconName, label: &str, value: &str, palette: Palette) -> gpui::Div {
    div()
        .min_w(px(170.))
        .flex_1()
        .flex()
        .items_center()
        .gap_3()
        .child(
            div()
                .size(px(26.))
                .flex_none()
                .rounded(px(7.))
                .bg(palette.panel)
                .text_color(palette.accent)
                .flex()
                .items_center()
                .justify_center()
                .child(Icon::new(icon)),
        )
        .child(
            div()
                .child(
                    div()
                        .text_size(px(10.))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(palette.subtle)
                        .child(label.to_uppercase()),
                )
                .child(
                    div()
                        .pt_1()
                        .text_size(px(10.))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child(value.to_string()),
                ),
        )
}

fn connection_metric(label: &str, value: String, palette: Palette) -> gpui::Div {
    div()
        .min_w_0()
        .p_3()
        .rounded(px(8.))
        .bg(palette.surface)
        .child(
            div()
                .text_size(px(10.))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(palette.subtle)
                .child(label.to_uppercase()),
        )
        .child(
            div()
                .pt_2()
                .font_family(FONT_MONO)
                .text_size(px(10.))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_ellipsis()
                .child(value),
        )
}

fn connection_status_row(label: &str, value: String, tone: Tone, palette: Palette) -> gpui::Div {
    div()
        .min_h(px(44.))
        .py_3()
        .border_b_1()
        .border_color(palette.border)
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .child(
            div()
                .text_size(px(11.))
                .text_color(palette.muted)
                .child(label.to_string()),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(status_dot(tone))
                .child(
                    div()
                        .font_family(FONT_MONO)
                        .text_size(px(9.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(tone_color(tone, palette))
                        .child(value),
                ),
        )
}

fn status_pill(status: &str, palette: Palette) -> gpui::Div {
    let color = match status {
        "healthy" | "ok" => palette.good,
        "warning" | "quiet" => palette.warn,
        "error" => palette.bad,
        _ => palette.muted,
    };
    div()
        .px_2()
        .py_1()
        .rounded(px(5.))
        .bg(color.opacity(0.14))
        .text_color(color)
        .font_family(FONT_MONO)
        .text_size(px(9.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .child(status.to_uppercase())
}

fn empty_resource_state(workers_only: bool, palette: Palette) -> gpui::Div {
    div()
        .min_h(px(280.))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .child(
            div()
                .size(px(48.))
                .rounded(px(14.))
                .bg(palette.accent_soft)
                .text_color(palette.accent)
                .flex()
                .items_center()
                .justify_center()
                .child(Icon::new(if workers_only {
                    IconName::SquareTerminal
                } else {
                    IconName::Inbox
                })),
        )
        .child(
            div()
                .pt_4()
                .text_size(px(15.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(if workers_only {
                    "No Workers match this view"
                } else {
                    "No resources match this view"
                }),
        )
        .child(
            div()
                .pt_2()
                .max_w(px(360.))
                .text_center()
                .text_size(px(12.))
                .line_height(px(18.))
                .text_color(palette.muted)
                .child(
                    "Change the search or status filter, or sync the account to refresh inventory.",
                ),
        )
}

fn settings_preference_row(
    icon: IconName,
    title: &str,
    detail: &str,
    action: AnyElement,
    palette: Palette,
) -> gpui::Div {
    div()
        .min_h(px(64.))
        .py_3()
        .border_b_1()
        .border_color(palette.border)
        .flex()
        .flex_wrap()
        .items_center()
        .gap_3()
        .child(
            div()
                .size(px(32.))
                .flex_none()
                .rounded(px(8.))
                .bg(palette.surface)
                .text_color(palette.muted)
                .flex()
                .items_center()
                .justify_center()
                .child(Icon::new(icon)),
        )
        .child(
            div()
                .min_w(px(170.))
                .flex_grow()
                .child(
                    div()
                        .text_size(px(11.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(title.to_string()),
                )
                .child(
                    div()
                        .pt_1()
                        .text_size(px(10.))
                        .text_color(palette.muted)
                        .child(detail.to_string()),
                ),
        )
        .child(action)
}

fn metric_rows<const N: usize>(rows: [(&str, String); N], palette: Palette) -> gpui::Div {
    div()
        .pt_2()
        .flex()
        .flex_col()
        .children(rows.into_iter().map(|(label, value)| {
            div()
                .py_3()
                .border_b_1()
                .border_color(palette.border)
                .flex()
                .justify_between()
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(palette.muted)
                        .child(label.to_string()),
                )
                .child(
                    div()
                        .font_family(FONT_MONO)
                        .text_size(px(12.))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child(value),
                )
        }))
}

fn error_banner(error: UiError, palette: Palette, cx: &mut Context<CedarApp>) -> gpui::Div {
    let red = palette.bad;
    div()
        .px_4()
        .py_3()
        .rounded_lg()
        .border_1()
        .border_color(red.opacity(0.48))
        .bg(red.opacity(0.08))
        .flex()
        .items_start()
        .gap_3()
        .child(
            div()
                .pt(px(1.))
                .flex_none()
                .text_color(red)
                .child(Icon::new(IconName::TriangleAlert)),
        )
        .child(
            div()
                .min_w_0()
                .child(
                    div()
                        .text_size(px(11.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(palette.foreground)
                        .child(error.title),
                )
                .child(
                    div()
                        .pt_1()
                        .text_size(px(10.))
                        .line_height(px(15.))
                        .text_color(palette.muted)
                        .child(error.message),
                ),
        )
        .child(
            Button::new("dismiss-error")
                .ghost()
                .icon(IconName::Close)
                .tooltip("Dismiss error")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.error = None;
                    cx.notify();
                })),
        )
}

fn static_notice(notice: UiNotice, palette: Palette, cx: &mut Context<CedarApp>) -> gpui::Div {
    let tone = if notice.error {
        palette.bad
    } else {
        palette.good
    };
    div()
        .absolute()
        .top_4()
        .right_4()
        .w(px(340.))
        .px_4()
        .py_3()
        .rounded(px(10.))
        .border_1()
        .border_color(tone.opacity(0.5))
        .bg(palette.raised)
        .shadow(palette.panel_shadow())
        .flex()
        .items_center()
        .gap_3()
        .child(
            Icon::new(if notice.error {
                IconName::TriangleAlert
            } else {
                IconName::CircleCheck
            })
            .text_color(tone),
        )
        .child(
            div()
                .min_w_0()
                .flex_grow()
                .text_size(px(11.))
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(notice.message),
        )
        .child(
            Button::new("dismiss-static-notice")
                .ghost()
                .compact()
                .icon(IconName::Close)
                .tooltip("Dismiss")
                .on_click(cx.listener(CedarApp::dismiss_notice)),
        )
}

fn record_inspector_history(history: &mut Vec<String>, index: &mut Option<usize>, key: &str) {
    if let Some(index) = *index {
        history.truncate(index + 1);
    } else {
        history.clear();
    }
    if history.last().is_none_or(|current| current != key) {
        history.push(key.into());
        if history.len() > INSPECTOR_HISTORY_LIMIT {
            history.remove(0);
        }
    }
    *index = history.len().checked_sub(1);
}

fn find_inspector_history_target(
    history: &[String],
    current: Option<usize>,
    direction: isize,
    mut is_valid: impl FnMut(&str) -> bool,
) -> Option<usize> {
    if direction == 0 {
        return None;
    }
    let mut index = current? as isize + direction;
    while index >= 0 && (index as usize) < history.len() {
        if is_valid(&history[index as usize]) {
            return Some(index as usize);
        }
        index += direction;
    }
    None
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use crate::{
        audit::{AuditFinding, Tone},
        backend::{ResourceBinding, ResourceRow},
    };

    use super::{
        PersistedWindowState, TOPOLOGY_NODE_WIDTH, VisualQaConfig, VisualQaScenario,
        WorkspaceState, build_topology_layout, evidence_resource_key,
        find_inspector_history_target, finding_resource_keys, fit_topology_view,
        record_inspector_history, usage_trend_summary, visual_qa_snapshot,
    };

    fn resource(kind: &str, id: impl Into<String>, bindings: Vec<ResourceBinding>) -> ResourceRow {
        let id = id.into();
        ResourceRow {
            name: id.clone(),
            id,
            kind: kind.into(),
            status: "healthy".into(),
            primary_metric: String::new(),
            secondary_metric: String::new(),
            updated_at: None,
            bindings: Some(bindings),
            observability: None,
        }
    }

    fn binding(kind: &str, id: impl Into<String>) -> ResourceBinding {
        ResourceBinding {
            name: "DATA".into(),
            binding_type: Some(kind.into()),
            resource_kind: Some(kind.into()),
            resource_id: Some(id.into()),
            resource_name: None,
        }
    }

    #[test]
    fn inspector_history_truncates_forward_entries_after_new_selection() {
        let mut history = Vec::new();
        let mut index = None;
        for key in ["worker-a", "worker-b", "worker-c"] {
            record_inspector_history(&mut history, &mut index, key);
        }

        index = Some(0);
        record_inspector_history(&mut history, &mut index, "worker-d");

        assert_eq!(history, ["worker-a", "worker-d"]);
        assert_eq!(index, Some(1));
    }

    #[test]
    fn inspector_history_skips_resources_missing_from_the_snapshot() {
        let history = vec!["worker-a".into(), "worker-b".into(), "worker-c".into()];

        let target = find_inspector_history_target(&history, Some(2), -1, |key| key == "worker-a");

        assert_eq!(target, Some(0));
    }

    #[test]
    fn workspace_state_sanitizes_stale_preferences() {
        let state = WorkspaceState {
            version: 44,
            active_section: "removed-section".into(),
            range: "forever".into(),
            resource_view_mode: "cards".into(),
            status_filter: "broken".into(),
            resource_query: "api".into(),
            inspector_tab: "logs".into(),
            selected_resource: Some("worker-api".into()),
        }
        .sanitized();

        assert_eq!(state.version, 1);
        assert_eq!(state.active_section, "overview");
        assert_eq!(state.range, "24h");
        assert_eq!(state.resource_view_mode, "table");
        assert_eq!(state.status_filter, "all");
        assert_eq!(state.inspector_tab, "overview");
        assert_eq!(state.resource_query, "api");
        assert_eq!(state.selected_resource.as_deref(), Some("worker-api"));
    }

    #[test]
    fn persisted_window_state_round_trips_maximized_restore_bounds() {
        let state = PersistedWindowState {
            x: -1440.,
            y: 18.,
            width: 1360.,
            height: 900.,
            maximized: true,
        };

        let value = serde_json::to_string(&state).expect("window state should serialize");
        let restored: PersistedWindowState =
            serde_json::from_str(&value).expect("window state should deserialize");

        assert_eq!(restored.x, state.x);
        assert_eq!(restored.width, state.width);
        assert!(restored.maximized);
    }

    #[test]
    fn visual_qa_options_parse_a_fixed_scenario_theme_and_viewport() {
        let args = [
            "--visual-qa",
            "workers-inspector",
            "--theme",
            "light",
            "--viewport",
            "1120x720",
        ]
        .map(str::to_string);

        let config = VisualQaConfig::from_args(&args)
            .expect("visual QA options should parse")
            .expect("visual QA should be enabled");

        assert_eq!(config.scenario, VisualQaScenario::WorkersInspector);
        assert!(!config.dark);
        assert_eq!((config.width, config.height), (1120., 720.));
    }

    #[test]
    fn visual_qa_fixture_covers_inventory_health_and_usage_states() {
        let snapshot = visual_qa_snapshot();

        assert_eq!(
            snapshot
                .account
                .as_ref()
                .map(|account| account.name.as_str()),
            Some("Artor Studio")
        );
        assert!(snapshot.resources.len() >= 10);
        assert!(
            snapshot
                .resources
                .iter()
                .any(|resource| resource.status != "healthy")
        );
        assert!(snapshot.usage_panels.len() >= 5);
        assert!(
            snapshot
                .health
                .iter()
                .any(|service| service.status == "warn")
        );
        assert!(!snapshot.collector.endpoints.is_empty());
    }

    #[test]
    fn usage_trend_summary_uses_first_and_latest_real_points() {
        assert_eq!(
            usage_trend_summary(&[100, 112, 125]).map(|value| value.to_string()),
            Some("+25.0%".into())
        );
        assert_eq!(
            usage_trend_summary(&[80, 60, 40]).map(|value| value.to_string()),
            Some("-50.0%".into())
        );
        assert_eq!(
            usage_trend_summary(&[0, 0]).map(|value| value.to_string()),
            Some("flat".into())
        );
        assert_eq!(
            usage_trend_summary(&[0, 4]).map(|value| value.to_string()),
            Some("new".into())
        );
        assert_eq!(usage_trend_summary(&[]), None);
        assert_eq!(usage_trend_summary(&[12]), None);
    }

    #[test]
    fn finding_evidence_resolves_exact_resources_and_ignores_collector_text() {
        let resources = vec![
            resource("worker", "api", vec![]),
            resource("worker", "api-v2", vec![]),
        ];
        let finding = AuditFinding {
            title: "Worker errors".into(),
            detail: String::new(),
            tone: Tone::Warn,
            section: None,
            action: None,
            evidence: vec![
                "api-v2 (worker, warning)".into(),
                "GET /accounts/id/workers/scripts / 403".into(),
                "api-v2 (worker, warning)".into(),
            ],
        };

        assert_eq!(
            evidence_resource_key(&finding.evidence[0], &resources),
            Some("worker-api-v2".into())
        );
        assert_eq!(
            finding_resource_keys(&finding, &resources),
            ["worker-api-v2"]
        );
    }

    #[test]
    fn topology_layout_is_stable_and_resolves_resource_bindings() {
        let resources = vec![
            resource("r2", "assets", vec![]),
            resource("worker", "api", vec![binding("d1", "app-db")]),
            resource("d1", "app-db", vec![]),
            resource("worker", "web", vec![binding("r2", "assets")]),
        ];

        let first = build_topology_layout(&resources);
        let second = build_topology_layout(&resources);

        assert_eq!(first, second);
        assert_eq!(first.nodes.len(), 4);
        assert_eq!(first.edges.len(), 2);
        assert_eq!(
            first
                .lanes
                .iter()
                .map(|lane| lane.label)
                .collect::<Vec<_>>(),
            ["COMPUTE", "DATA & STATE", "STORAGE & EDGE"]
        );
        assert!(first.edges.iter().all(|edge| {
            first.nodes[edge.from].kind == "worker"
                && matches!(first.nodes[edge.to].kind.as_str(), "d1" | "r2")
        }));
        let minimum = fit_topology_view(&first, 872., 410.);
        let wide = fit_topology_view(&first, 1440., 410.);
        assert!(minimum.0 < wide.0);
        assert!(wide.1 > minimum.1);
        let minimum_left = first
            .nodes
            .iter()
            .map(|node| minimum.1 + node.x * minimum.0)
            .fold(f32::INFINITY, f32::min);
        let minimum_right = first
            .nodes
            .iter()
            .map(|node| minimum.1 + node.x * minimum.0 + TOPOLOGY_NODE_WIDTH)
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(minimum_left >= 0.);
        assert!(minimum_right <= 872.);
    }

    #[test]
    fn topology_layout_handles_large_inventories_without_quadratic_work() {
        let resource_count = 5_000;
        let mut resources = Vec::with_capacity(resource_count);
        for index in 0..resource_count / 2 {
            resources.push(resource(
                "worker",
                format!("worker-{index}"),
                vec![binding("d1", format!("db-{index}"))],
            ));
            resources.push(resource("d1", format!("db-{index}"), vec![]));
        }

        let started = Instant::now();
        let layout = build_topology_layout(&resources);
        let elapsed = started.elapsed();
        println!(
            "topology layout: {} nodes / {} edges in {:.2?}",
            layout.nodes.len(),
            layout.edges.len(),
            elapsed
        );

        assert_eq!(layout.nodes.len(), resource_count);
        assert_eq!(layout.edges.len(), resource_count / 2);
        assert!(elapsed < Duration::from_secs(1));
    }
}
