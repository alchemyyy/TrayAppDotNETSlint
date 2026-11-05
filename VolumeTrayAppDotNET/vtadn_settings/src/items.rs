//! The page model: every page is a flat list of [`Item`]s (headers, supporting text, cards). The
//! Slint side renders `Item::view`; edits come back by row index and run `Item::action`. Port of
//! the card builders in `CommonBindings.cs` / `Cards.cs` (BoolCard, IntCard, StringComboCard,
//! PairBoolCard, PairColumnHeader, Single/VariantColorCard, Maybe) and the search roles of
//! `SettingsSearchMetadata.cs`.

use std::rc::Rc;

use slint::{Color, ModelRc, SharedString, VecModel};
use tadn_common::hotkey::HotkeyAction;
use tadn_common::theme::Argb;
use vtadn_model::settings::{AppSettings, SliderThumbOption, SliderThumbShape};

use crate::SettingsPage;
use crate::strings as s;
use crate::ui::{ComboOption, HotkeyEntry, ItemKind, SettingsItem};

pub(crate) type SetBool = fn(&mut AppSettings, bool);
pub(crate) type SetInt = fn(&mut AppSettings, i32);
/// Combo edit: applies the option at an index.
pub(crate) type ComboSetter = Rc<dyn Fn(&mut AppSettings, usize)>;

/// `SettingsSearchRole`: how the search view treats a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    PageHeader,
    /// `SubsectionHeader` (also `PairColumnHeader`).
    Section,
    /// A control that is neither header nor card (description text, About rows, known issues).
    Supporting,
    /// A searchable card (`SettingsSearchMetadata.MarkCard`).
    Card,
}

/// Which color a color card edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ColorTarget {
    TextColor,
    BackgroundColor,
    TrayIconColor,
    MeterPeak,
    MeterPeakStereo,
}

impl ColorTarget {
    /// Light/dark override pair (`VariantColorCard`) versus one color (`SingleColorCard`).
    pub(crate) fn is_variant(self) -> bool {
        matches!(self, Self::TextColor | Self::BackgroundColor | Self::TrayIconColor)
    }
}

pub(crate) enum Action {
    None,
    Bool(SetBool),
    Int(SetInt),
    Combo(ComboSetter),
    Pair {
        left: Option<SetBool>,
        right: Option<SetBool>,
    },
    Color(ColorTarget),
    SliderThumb(Vec<SliderThumbOption>),
    Startup,
    Language(Vec<&'static str>),
    RenderingBackend,
    Hotkey(HotkeyAction),
    OpenFolder,
    OpenLink(&'static str),
    /// Search results page heading: show the page and highlight the matched cards.
    Navigate(SettingsPage, Vec<String>),
}

pub(crate) struct Item {
    /// Stable identity (model diffing, search highlight).
    pub id: String,
    pub role: Role,
    pub view: SettingsItem,
    pub action: Action,
    /// Resx `*_SearchKeywords` values (and other search-only text).
    pub keywords: Vec<String>,
}

impl Item {
    /// Search body: every card text except the title (`SettingsSearchView.ReadDocument`).
    pub(crate) fn search_body(&self) -> String {
        let v = &self.view;
        let mut parts: Vec<String> = Vec::new();
        let mut push = |text: &str| {
            if !text.trim().is_empty() && !text.eq_ignore_ascii_case(v.title.as_str()) {
                parts.push(text.to_owned());
            }
        };
        push(v.description.as_str());
        match v.kind {
            ItemKind::NumberCard => push(v.suffix.as_str()),
            ItemKind::ComboCard => {
                if let Ok(index) = usize::try_from(v.selected)
                    && let Some(option) = slint::Model::row_data(&v.options, index)
                {
                    push(option.text.as_str());
                }
            }
            ItemKind::ColorCard => push(v.reset_text.as_str()),
            ItemKind::ButtonCard => push(v.button_text.as_str()),
            ItemKind::HotkeyCard => push(v.add_text.as_str()),
            _ => {}
        }
        parts.join(". ")
    }
}

/// `ComboOption` for a plain text entry.
pub(crate) fn text_option(text: &str) -> ComboOption {
    ComboOption { text: text.into(), x_scale: 1.0, ..Default::default() }
}

/// `ComboOption` with the slider-thumb preview (`SliderThumbComboContent`).
pub(crate) fn thumb_option(option: &SliderThumbOption, label: &str) -> ComboOption {
    ComboOption {
        text: label.into(),
        glyph: option.glyph.as_str().into(),
        glyph_font: option.font_family.as_str().into(),
        glyph_size: option.font_size.max(1.0) as f32,
        preview_width: option.width.max(1.0) as f32,
        preview_height: option.height.max(1.0) as f32,
        x_scale: if (option.x_scale - 1.0).abs() > 0.001 { option.x_scale as f32 } else { 1.0 },
        capsule: option.shape == SliderThumbShape::Capsule,
    }
}

/// Whether a row has a keyboard focus stop (ControlMap: every card control is a tab stop; disabled
/// cards, headers, supporting text and hidden pair sides are not).
pub(crate) fn is_focus_stop(view: &SettingsItem) -> bool {
    match view.kind {
        ItemKind::ToggleCard
        | ItemKind::NumberCard
        | ItemKind::ComboCard
        | ItemKind::ColorCard
        | ItemKind::ButtonCard
        | ItemKind::HotkeyCard => view.enabled,
        ItemKind::PairCard => {
            view.enabled && ((view.has_left && view.show_left) || (view.has_right && view.show_right))
        }
        ItemKind::AboutRow => view.is_link,
        _ => false,
    }
}

/// The row whose first control takes focus when a page is opened from the keyboard (Space/Right
/// on its row) or the search results are entered (Enter in the search box): the first row, in
/// card order, with a focus stop.
pub(crate) fn first_focus_row(items: &[Item]) -> Option<usize> {
    items.iter().position(|item| is_focus_stop(&item.view))
}

pub(crate) fn model<T: Clone + 'static>(values: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(values))
}

pub(crate) fn slint_color(argb: Argb) -> Color {
    Color::from_argb_encoded(argb.0)
}

pub(crate) fn base_view(kind: ItemKind, title: &str, description: &str) -> SettingsItem {
    SettingsItem {
        kind,
        title: title.into(),
        description: description.into(),
        enabled: true,
        show_left: true,
        show_right: true,
        selected: -1,
        modifier_selected: -1,
        ..Default::default()
    }
}

/// Collects one page's rows. Card helpers mirror the C# `BoolCard`/`IntCard`/... signatures; a
/// `Maybe(false, card)` is simply not added (hidden cards are invisible to search too).
pub(crate) struct PageBuilder {
    pub items: Vec<Item>,
    /// Card ids already used on this page (ids must be unique per page).
    prefix: &'static str,
}

impl PageBuilder {
    pub(crate) fn new(prefix: &'static str) -> Self {
        Self { items: Vec::new(), prefix }
    }

    fn id(&self, id: &str) -> String {
        format!("{}.{}", self.prefix, id)
    }

    pub(crate) fn push(&mut self, id: &str, role: Role, view: SettingsItem, action: Action, keywords: &[&str]) {
        let id = self.id(id);
        debug_assert!(!self.items.iter().any(|i| i.id == id), "duplicate settings item id {id}");
        self.items.push(Item {
            id,
            role,
            view,
            action,
            keywords: keywords.iter().filter(|k| !k.trim().is_empty()).map(|k| (*k).to_owned()).collect(),
        });
    }

    /// `PageStack(title)`: the page's 22px header.
    pub(crate) fn header(&mut self, title: &str) {
        self.push("header", Role::PageHeader, base_view(ItemKind::PageHeader, title, ""), Action::None, &[]);
    }

    /// `TrayAppDotNETSettingsUI.SubsectionHeader`.
    pub(crate) fn section(&mut self, id: &str, title: &str) {
        self.push(id, Role::Section, base_view(ItemKind::Subsection, title, ""), Action::None, &[]);
    }

    /// `DescriptionText` as a page row; `margin_bottom` is the C# bottom margin.
    pub(crate) fn text(&mut self, id: &str, text: &str, margin_bottom: f32) {
        let mut view = base_view(ItemKind::Description, "", text);
        view.margin_bottom = margin_bottom;
        self.push(id, Role::Supporting, view, Action::None, &[]);
    }

    /// `PairColumnHeader`: a subsection header with the two column captions.
    pub(crate) fn pair_header(&mut self, id: &str, title: &str) {
        let mut view = base_view(ItemKind::PairHeader, title, "");
        view.left_text = s::Settings_Common_Playback.into();
        view.right_text = s::Settings_Common_Recording.into();
        self.push(id, Role::Section, view, Action::None, &[]);
    }

    pub(crate) fn toggle(
        &mut self,
        id: &str,
        title: &str,
        description: &str,
        value: bool,
        set: SetBool,
        keywords: &[&str],
    ) {
        let mut view = base_view(ItemKind::ToggleCard, title, description);
        view.checked = value;
        self.push(id, Role::Card, view, Action::Bool(set), keywords);
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn number(
        &mut self,
        id: &str,
        title: &str,
        description: &str,
        value: i32,
        range: (i32, i32),
        suffix: &str,
        set: SetInt,
        keywords: &[&str],
    ) {
        let mut view = base_view(ItemKind::NumberCard, title, description);
        view.number = value;
        view.minimum = range.0;
        view.maximum = range.1;
        view.suffix = suffix.into();
        self.push(id, Role::Card, view, Action::Int(set), keywords);
    }

    /// `StringComboCard`: autosized to the selected item. Returns the row index for tweaks
    /// (`IsEnabled`).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn combo<T: Copy + PartialEq + 'static>(
        &mut self,
        id: &str,
        title: &str,
        description: &str,
        options: &[(T, &str)],
        value: T,
        set: fn(&mut AppSettings, T),
        keywords: &[&str],
    ) -> usize {
        let mut view = base_view(ItemKind::ComboCard, title, description);
        view.options = model(options.iter().map(|(_, text)| text_option(text)).collect());
        view.selected = options.iter().position(|(v, _)| *v == value).map_or(-1, |i| i as i32);
        let values: Vec<T> = options.iter().map(|(v, _)| *v).collect();
        let action = Action::Combo(Rc::new(move |settings: &mut AppSettings, index: usize| {
            if let Some(v) = values.get(index) {
                set(settings, *v);
            }
        }));
        self.push(id, Role::Card, view, action, keywords);
        self.items.len() - 1
    }

    /// `PairBoolCard`: a side exists when it has a value and setter; `show_*` hides an existing
    /// side's toggle while keeping its column.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn pair(
        &mut self,
        id: &str,
        title: &str,
        description: &str,
        left: Option<(bool, SetBool)>,
        right: Option<(bool, SetBool)>,
        show_left: bool,
        show_right: bool,
        keywords: &[&str],
    ) {
        let mut view = base_view(ItemKind::PairCard, title, description);
        view.has_left = left.is_some();
        view.has_right = right.is_some();
        view.left_value = left.is_some_and(|(v, _)| v);
        view.right_value = right.is_some_and(|(v, _)| v);
        view.show_left = show_left;
        view.show_right = show_right;
        view.tooltip = format!("{} / {}", s::Settings_Common_Playback, s::Settings_Common_Recording).into();
        let action = Action::Pair { left: left.map(|(_, f)| f), right: right.map(|(_, f)| f) };
        self.push(id, Role::Card, view, action, keywords);
    }

    /// `SingleColorCard` / `VariantColorCard` (only the swatch of the effective mode is shown).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn color(
        &mut self,
        id: &str,
        title: &str,
        description: &str,
        color: Argb,
        unset: bool,
        tooltip: &str,
        target: ColorTarget,
        keywords: &[&str],
    ) {
        let mut view = base_view(ItemKind::ColorCard, title, description);
        view.color = slint_color(color);
        view.color_unset = unset;
        view.tooltip = tooltip.into();
        view.reset_text = s::Settings_Theme_Reset.into();
        self.push(id, Role::Card, view, Action::Color(target), keywords);
    }

    /// `Card(title, description, button)`.
    pub(crate) fn button(
        &mut self,
        id: &str,
        title: &str,
        description: &str,
        text: &str,
        action: Action,
        keywords: &[&str],
    ) {
        let mut view = base_view(ItemKind::ButtonCard, title, description);
        view.button_text = text.into();
        self.push(id, Role::Card, view, action, keywords);
    }

    pub(crate) fn last_mut(&mut self) -> &mut Item {
        let len = self.items.len();
        &mut self.items[len - 1]
    }
}

/// Hotkey card view data (built by `hotkeys.rs`).
pub(crate) struct HotkeyView {
    pub modifier_selected: i32,
    pub key_text: String,
    pub add_text: &'static str,
    pub add_enabled: bool,
    pub feedback: String,
    pub entries: Vec<HotkeyEntry>,
}

pub(crate) fn hotkey_view(
    title: &str,
    description: &str,
    data: HotkeyView,
    modifier_options: ModelRc<ComboOption>,
) -> SettingsItem {
    let mut view = base_view(ItemKind::HotkeyCard, title, description);
    view.modifier_options = modifier_options;
    view.modifier_selected = data.modifier_selected;
    view.key_text = data.key_text.into();
    view.add_text = data.add_text.into();
    view.add_enabled = data.add_enabled;
    view.feedback = data.feedback.into();
    view.entries = model(data.entries);
    view.delete_tooltip = SharedString::from(s::Settings_Hotkeys_DeleteHotkey_ToolTip);
    view
}
