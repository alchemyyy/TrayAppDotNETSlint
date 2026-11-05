//! Settings search view (port of `SettingsWindowSearch.cs` + `Settings/SettingsSearchView.cs`):
//! every page is built, cards are indexed with their page and section context, and the matches
//! are shown as live cards under their page and section headings, in page order.

use std::sync::LazyLock;

use tadn_common::search::{COMMON_SYNONYM_GROUPS, SearchEntry, SynonymMap, find_matches};

use crate::SettingsPage;
use crate::items::{Action, Item, Role, base_view};
use crate::pages::{BuildContext, PAGES, build_page, page_label};
use crate::strings as s;
use crate::ui::ItemKind;

/// Common + Volume synonym groups (`SettingsSearchSynonymMap.Parse`).
static SYNONYMS: LazyLock<SynonymMap> =
    LazyLock::new(|| SynonymMap::parse(&[COMMON_SYNONYM_GROUPS, s::SettingsWindow_SearchSynonymGroups_App]));

/// One searchable unit: a card, or the content of a section without cards (`ContentEntries`).
struct Document {
    page: usize,
    /// Item indices (within the page) shown when the document matches.
    items: Vec<usize>,
    entry: SearchEntry,
}

/// A page split into sections (`SearchSection`): the header row, supporting rows, documents.
struct Section {
    header: Option<usize>,
    supporting: Vec<usize>,
    cards: Vec<usize>,
}

fn sections(items: &[Item]) -> Vec<Section> {
    let mut sections = vec![Section { header: None, supporting: Vec::new(), cards: Vec::new() }];
    for (index, item) in items.iter().enumerate() {
        match item.role {
            Role::PageHeader => {}
            Role::Section => sections.push(Section { header: Some(index), supporting: Vec::new(), cards: Vec::new() }),
            Role::Supporting => sections.last_mut().map_or((), |s| s.supporting.push(index)),
            Role::Card => sections.last_mut().map_or((), |s| s.cards.push(index)),
        }
    }
    sections
}

fn item_text(item: &Item) -> String {
    let v = &item.view;
    [v.title.as_str(), v.description.as_str()].iter().filter(|t| !t.is_empty()).copied().collect::<Vec<_>>().join(". ")
}

/// Search entries of one page (exposed for the metadata test).
fn page_documents(page_index: usize, page: SettingsPage, items: &[Item]) -> Vec<Document> {
    let label = page_label(page);
    let mut documents = Vec::new();
    for section in sections(items) {
        let header = section.header.map(|i| items[i].view.title.to_string()).unwrap_or_default();
        let context: Vec<String> = section.supporting.iter().map(|&i| item_text(&items[i])).collect();
        let section_text =
            std::iter::once(header).chain(context).filter(|t| !t.is_empty()).collect::<Vec<_>>().join(". ");
        if section.cards.is_empty() {
            // FinalizeSections: a section of supporting controls only is one document.
            if section.supporting.is_empty() {
                continue;
            }
            let texts: Vec<String> = section.supporting.iter().map(|&i| item_text(&items[i])).collect();
            let title = texts.first().cloned().unwrap_or_default();
            documents.push(Document {
                page: page_index,
                items: section.supporting.clone(),
                entry: SearchEntry {
                    title,
                    description: texts[1..].join(". "),
                    keywords: Vec::new(),
                    page: label.to_owned(),
                    section: section.header.map(|i| items[i].view.title.to_string()).unwrap_or_default(),
                },
            });
            continue;
        }
        for &card in &section.cards {
            let item = &items[card];
            documents.push(Document {
                page: page_index,
                items: vec![card],
                entry: SearchEntry {
                    title: item.view.title.to_string(),
                    description: item.search_body(),
                    keywords: item.keywords.clone(),
                    page: label.to_owned(),
                    section: section_text.clone(),
                },
            });
        }
    }
    documents
}

/// The search entries of every page, in order (for tests: each card has search metadata).
#[cfg(test)]
pub(crate) fn all_entries(ctx: &BuildContext<'_>) -> Vec<(SettingsPage, String, SearchEntry)> {
    let mut out = Vec::new();
    for (page_index, page) in PAGES.iter().enumerate() {
        let items = build_page(*page, ctx);
        for document in page_documents(page_index, *page, &items) {
            let id = items[document.items[0]].id.clone();
            out.push((*page, id, document.entry));
        }
    }
    out
}

/// Builds the "Search results" page for `query` (already trimmed, non-empty).
pub(crate) fn build_results(query: &str, ctx: &BuildContext<'_>) -> Vec<Item> {
    let pages: Vec<Vec<Item>> = PAGES.iter().map(|page| build_page(*page, ctx)).collect();
    let documents: Vec<Document> =
        PAGES.iter().enumerate().flat_map(|(i, page)| page_documents(i, *page, &pages[i])).collect();
    let entries: Vec<SearchEntry> = documents.iter().map(|d| d.entry.clone()).collect();
    let matched = find_matches(query, &entries, Some(&SYNONYMS));

    // Visible item indices per page.
    let mut shown: Vec<Vec<bool>> = pages.iter().map(|items| vec![false; items.len()]).collect();
    for &document in &matched {
        for &item in &documents[document].items {
            shown[documents[document].page][item] = true;
        }
    }

    let mut results = Vec::new();
    results.push(Item {
        id: "search.header".to_owned(),
        role: Role::PageHeader,
        view: base_view(ItemKind::PageHeader, s::SettingsWindow_SearchResults, ""),
        action: Action::None,
        keywords: Vec::new(),
    });
    if matched.is_empty() {
        let mut view =
            base_view(ItemKind::Description, "", &s::SettingsWindow_SearchNoMatchesFormat.replace("{0}", query));
        view.margin_bottom = 0.0;
        results.push(Item {
            id: "search.status".to_owned(),
            role: Role::Supporting,
            view,
            action: Action::None,
            keywords: Vec::new(),
        });
        return results;
    }

    for (page_index, items) in pages.into_iter().enumerate() {
        let visible = &shown[page_index];
        if !visible.iter().any(|v| *v) {
            continue;
        }
        let page = PAGES[page_index];
        let matched_ids: Vec<String> = items
            .iter()
            .zip(visible)
            .filter(|(item, v)| **v && item.role == Role::Card)
            .map(|(i, _)| i.id.clone())
            .collect();
        let mut link = base_view(ItemKind::PageLink, page_label(page), "");
        link.tooltip = page_label(page).into();
        results.push(Item {
            id: format!("search.page.{page_index}"),
            role: Role::PageHeader,
            view: link,
            action: Action::Navigate(page, matched_ids),
            keywords: Vec::new(),
        });
        let page_sections = sections(&items);
        let mut items: Vec<Option<Item>> = items.into_iter().map(Some).collect();
        for section in page_sections {
            let section_has_match = section.cards.iter().chain(&section.supporting).any(|&i| visible[i]);
            if !section_has_match {
                continue;
            }
            // Section header and its supporting text show whenever the section has a match.
            let mut rows: Vec<usize> = section.header.into_iter().collect();
            rows.extend(section.supporting.iter().copied());
            rows.extend(section.cards.iter().copied().filter(|&i| visible[i]));
            rows.sort_unstable();
            results.extend(rows.into_iter().filter_map(|row| items[row].take()));
        }
    }
    results
}
