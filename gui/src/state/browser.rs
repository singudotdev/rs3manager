//! The folder on screen: which bucket and prefix, its rows as loaded so far, the filter typed over them, and which
//! rows are selected. Listings arrive asynchronously, so each request carries a generation and a stale answer (the
//! user moved on) is dropped instead of overwriting the folder now shown.

use super::format::name_in;
use r3manager_core::s3::{ObjectEntry, Page};
use std::collections::BTreeSet;

/// A row: a folder (its full prefix, ending in `/`) or an object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// A folder.
    Folder(String),
    /// An object.
    Object(ObjectEntry),
}

impl Row {
    /// The row's full key (a folder's prefix).
    pub fn key(&self) -> &str {
        match self {
            Self::Folder(prefix) => prefix,
            Self::Object(object) => &object.key,
        }
    }
}

/// The folder shown and what's selected in it.
#[derive(Debug, Default)]
pub struct Browser {
    /// The bucket open, if any.
    bucket: Option<String>,
    /// The folder open in it, `""` for the top.
    prefix: String,
    /// Its rows loaded so far: folders, then objects.
    rows: Vec<Row>,
    /// The token for the next page, while there's more.
    next: Option<String>,
    /// Bumped by every navigation; a listing answers for the generation it was asked in.
    generation: u64,
    /// Whether a listing is under way.
    loading: bool,
    /// The filter typed: rows whose name contains it, ignoring case.
    filter: String,
    /// The keys selected.
    selected: BTreeSet<String>,
    /// The last row clicked without shift: where a shift-click's range starts.
    anchor: Option<String>,
}

impl Browser {
    /// The bucket open.
    pub fn bucket(&self) -> Option<&str> {
        self.bucket.as_deref()
    }

    /// The folder open.
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// Whether a listing is under way.
    pub fn loading(&self) -> bool {
        self.loading
    }

    /// Whether more rows can be loaded.
    pub fn has_more(&self) -> bool {
        self.next.is_some()
    }

    /// Opens folder `prefix` of `bucket` (or nothing), forgetting the rows and selection, and returns the generation
    /// its listing must answer for.
    pub fn open(&mut self, bucket: Option<String>, prefix: String) -> u64 {
        self.bucket = bucket;
        self.prefix = prefix;
        self.rows.clear();
        self.next = None;
        self.selected.clear();
        self.anchor = None;
        self.filter.clear();
        self.reload()
    }

    /// Starts listing the open folder again from its first page, keeping the selection of rows still there.
    pub fn reload(&mut self) -> u64 {
        self.generation += 1;
        self.loading = self.bucket.is_some();
        self.generation
    }

    /// Starts loading the next page; `None` when there's none or a listing is under way. Returns the generation and
    /// the token to list from.
    pub fn more(&mut self) -> Option<(u64, String)> {
        if self.loading {
            return None;
        }
        let token = self.next.clone()?;
        self.loading = true;
        Some((self.generation, token))
    }

    /// Takes a listing `page` asked for in `generation`: the first page (`first`) replaces the rows, a later one adds
    /// to them. A stale one is ignored. Returns whether it was taken.
    pub fn loaded(&mut self, generation: u64, first: bool, page: Page) -> bool {
        if generation != self.generation {
            return false;
        }
        if first {
            self.rows.clear();
        }
        self.rows.extend(page.folders.into_iter().map(Row::Folder));
        self.rows.extend(page.objects.into_iter().map(Row::Object));
        self.next = page.next;
        self.loading = false;
        let keys: BTreeSet<&str> = self.rows.iter().map(Row::key).collect();
        self.selected.retain(|k| keys.contains(k.as_str()));
        true
    }

    /// A listing asked for in `generation` failed: stops showing it as loading, if it's still current.
    pub fn failed(&mut self, generation: u64) {
        if generation == self.generation {
            self.loading = false;
        }
    }

    /// Sets the filter, keeping only selected rows it still shows.
    pub fn set_filter(&mut self, filter: &str) {
        self.filter = filter.trim().to_lowercase();
        let shown: BTreeSet<String> = self.rows().map(|r| r.key().to_owned()).collect();
        self.selected.retain(|k| shown.contains(k));
    }

    /// The rows the filter shows, folders first, in key order within each.
    pub fn rows(&self) -> impl Iterator<Item = &Row> {
        self.rows
            .iter()
            .filter(|r| self.filter.is_empty() || name_in(r.key(), &self.prefix).to_lowercase().contains(&self.filter))
    }

    /// The row shown at `index`.
    pub fn row(&self, index: usize) -> Option<&Row> {
        self.rows().nth(index)
    }

    /// How many rows the filter shows.
    pub fn len(&self) -> usize {
        self.rows().count()
    }

    /// Whether `key` is selected.
    pub fn is_selected(&self, key: &str) -> bool {
        self.selected.contains(key)
    }

    /// The selected rows, in the order shown.
    pub fn selection(&self) -> Vec<&Row> {
        self.rows().filter(|r| self.selected.contains(r.key())).collect()
    }

    /// The selected object when exactly one row is selected and it's an object: the one the details panel shows.
    pub fn single_object(&self) -> Option<&ObjectEntry> {
        match self.selection().as_slice() {
            [Row::Object(object)] => Some(object),
            _ => None,
        }
    }

    /// A click on the row of `key`: alone selects only it, with `toggle` (ctrl/cmd) adds or removes it, with `range`
    /// (shift) selects every row from the last plain click to it.
    pub fn click(&mut self, key: &str, toggle: bool, range: bool) {
        if range && let Some(anchor) = self.anchor.clone() {
            let keys: Vec<String> = self.rows().map(|r| r.key().to_owned()).collect();
            if let (Some(a), Some(b)) = (keys.iter().position(|k| *k == anchor), keys.iter().position(|k| k == key)) {
                if !toggle {
                    self.selected.clear();
                }
                self.selected.extend(keys[a.min(b)..=a.max(b)].iter().cloned());
                return;
            }
        }
        if toggle {
            if !self.selected.remove(key) {
                self.selected.insert(key.to_owned());
            }
        } else {
            self.selected.clear();
            self.selected.insert(key.to_owned());
        }
        self.anchor = Some(key.to_owned());
    }

    /// Selects every row shown, or none when all are already selected.
    pub fn toggle_all(&mut self) {
        let shown: Vec<String> = self.rows().map(|r| r.key().to_owned()).collect();
        if shown.iter().all(|k| self.selected.contains(k)) {
            self.selected.clear();
        } else {
            self.selected.extend(shown);
        }
    }

    /// Selects nothing.
    pub fn clear_selection(&mut self) {
        self.selected.clear();
        self.anchor = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An object row for `key`.
    fn object(key: &str) -> ObjectEntry {
        ObjectEntry { key: key.to_owned(), size: 1, modified: None }
    }

    /// A page with `folders` and `objects`, and `next` as its token.
    fn page(folders: &[&str], objects: &[&str], next: Option<&str>) -> Page {
        Page {
            folders: folders.iter().map(|f| (*f).to_owned()).collect(),
            objects: objects.iter().map(|o| object(o)).collect(),
            next: next.map(str::to_owned),
        }
    }

    /// The keys shown.
    fn keys(b: &Browser) -> Vec<&str> {
        b.rows().map(Row::key).collect()
    }

    #[test]
    fn stale_listings_are_dropped_and_pages_add_up() {
        let mut b = Browser::default();
        let old = b.open(Some("bk".into()), "a/".into());
        let current = b.open(Some("bk".into()), "b/".into());
        assert!(!b.loaded(old, true, page(&[], &["a/x"], None)));
        assert!(b.loaded(current, true, page(&["b/d/"], &["b/1"], Some("t"))));
        assert!(b.has_more());
        let (generation, token) = b.more().unwrap();
        assert_eq!(token, "t");
        assert!(b.more().is_none(), "one page at a time");
        assert!(b.loaded(generation, false, page(&[], &["b/2"], None)));
        assert_eq!(keys(&b), ["b/d/", "b/1", "b/2"]);
        assert!(!b.has_more() && !b.loading());
    }

    #[test]
    fn filter_matches_names_not_the_prefix_ignoring_case() {
        let mut b = Browser::default();
        let g = b.open(Some("bk".into()), "photos/".into());
        b.loaded(g, true, page(&["photos/Trip/"], &["photos/cat.PNG", "photos/dog.jpg"], None));
        b.set_filter("png");
        assert_eq!(keys(&b), ["photos/cat.PNG"]);
        b.set_filter("photos");
        assert_eq!(keys(&b), Vec::<&str>::new());
        b.set_filter("");
        assert_eq!(b.len(), 3);
    }

    #[test]
    fn clicks_select_toggle_and_range() {
        let mut b = Browser::default();
        let g = b.open(Some("bk".into()), String::new());
        b.loaded(g, true, page(&["d/"], &["a", "b", "c"], None));
        b.click("a", false, false);
        assert_eq!(b.single_object().map(|o| o.key.as_str()), Some("a"));
        b.click("c", false, true);
        assert_eq!(b.selection().iter().map(|r| r.key()).collect::<Vec<_>>(), ["a", "b", "c"]);
        b.click("b", true, false);
        assert_eq!(b.selection().iter().map(|r| r.key()).collect::<Vec<_>>(), ["a", "c"]);
        assert!(b.single_object().is_none());
        b.toggle_all();
        assert_eq!(b.selection().len(), 4);
        b.toggle_all();
        assert_eq!(b.selection(), Vec::<&Row>::new());
    }

    #[test]
    fn a_reload_keeps_the_selection_of_rows_still_there() {
        let mut b = Browser::default();
        let g = b.open(Some("bk".into()), String::new());
        b.loaded(g, true, page(&[], &["a", "b"], None));
        b.click("a", false, false);
        b.click("b", true, false);
        let g = b.reload();
        b.loaded(g, true, page(&[], &["b", "c"], None));
        assert_eq!(b.selection().iter().map(|r| r.key()).collect::<Vec<_>>(), ["b"]);
    }
}
