//! Global workspace navigation. Destinations restore selection, never perform writes.
use crate::app::Section;
use gpui_kit::{App, Window};
use std::rc::Rc;

type Restore = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(Clone)]
pub struct Badge {
    pub count: usize,
    pub progress: Option<f32>,
    pub tooltip: String,
}

#[derive(Clone)]
pub struct Destination {
    pub key: String,
    pub label: String,
    pub section: Option<Section>,
    pub badge: Option<Badge>,
    pub restore: Restore,
    pub close: Rc<dyn Fn(&mut App)>,
}
impl Destination {
    pub fn section(section: Option<Section>) -> Self {
        Self {
            key: section.map_or("dashboard", Section::id).into(),
            label: section.map_or("Dashboard", Section::label).into(),
            section,
            badge: None,
            restore: Rc::new(|_, _| {}),
            close: Rc::new(|_| {}),
        }
    }
}
#[derive(Default)]
pub struct Tabs {
    pub items: Vec<Destination>,
    pub active: Option<String>,
    pub closed: Vec<Destination>,
    back: Vec<String>,
    forward: Vec<String>,
}
impl Tabs {
    pub fn open(&mut self, destination: Destination) {
        let key = destination.key.clone();
        if let Some(existing) = self.items.iter_mut().find(|tab| tab.key == key) {
            *existing = destination;
        } else {
            self.items.push(destination);
        }
        self.select(&key);
    }
    pub fn select(&mut self, key: &str) {
        if self.active.as_deref() == Some(key) || !self.items.iter().any(|tab| tab.key == key) {
            return;
        }
        if let Some(old) = self.active.replace(key.into()) {
            self.back.push(old);
        }
        self.forward.clear();
    }
    pub fn current(&self) -> Option<Destination> {
        self.items
            .iter()
            .find(|tab| Some(&tab.key) == self.active.as_ref())
            .cloned()
    }
    pub fn cycle(&mut self, next: bool) {
        if self.items.is_empty() {
            return;
        }
        let index = self
            .items
            .iter()
            .position(|tab| Some(&tab.key) == self.active.as_ref())
            .unwrap_or(0);
        let index = if next {
            (index + 1) % self.items.len()
        } else {
            (index + self.items.len() - 1) % self.items.len()
        };
        self.select(&self.items[index].key.clone());
    }
    pub fn close(&mut self, key: &str) {
        let Some(index) = self.items.iter().position(|tab| tab.key == key) else {
            return;
        };
        self.closed.push(self.items.remove(index));
        self.back.retain(|item| item != key);
        self.forward.retain(|item| item != key);
        if self.active.as_deref() == Some(key) {
            self.active = self
                .items
                .get(index.min(self.items.len().saturating_sub(1)))
                .map(|tab| tab.key.clone());
        }
    }
    pub fn close_all(&mut self) {
        let keys: Vec<_> = self.items.iter().map(|d| d.key.clone()).collect();
        for key in keys {
            self.close(&key);
        }
    }
    pub fn retain(&mut self, key: &str, right: bool) {
        let Some(index) = self.items.iter().position(|tab| tab.key == key) else {
            return;
        };
        let closed: Vec<_> = self
            .items
            .iter()
            .enumerate()
            .filter(|(i, _)| if right { *i > index } else { *i != index })
            .map(|(_, tab)| tab.key.clone())
            .collect();
        for item in closed {
            self.close(&item);
        }
    }
    pub fn move_tab(&mut self, key: &str, delta: isize) {
        if let Some(index) = self.items.iter().position(|tab| tab.key == key) {
            let next = index.saturating_add_signed(delta).min(self.items.len() - 1);
            self.items.swap(index, next);
        }
    }
    pub fn travel(&mut self, forward: bool) {
        let (from, to) = if forward {
            (&mut self.forward, &mut self.back)
        } else {
            (&mut self.back, &mut self.forward)
        };
        while let Some(key) = from.pop() {
            if self.active.as_ref() == Some(&key) {
                continue;
            }
            if let Some(old) = self.active.replace(key) {
                to.push(old);
            }
            break;
        }
    }
    pub fn can_back(&self) -> bool {
        self.back
            .iter()
            .any(|key| self.active.as_ref() != Some(key))
    }
    pub fn can_forward(&self) -> bool {
        self.forward
            .iter()
            .any(|key| self.active.as_ref() != Some(key))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn tab(key: &str) -> Destination {
        let mut d = Destination::section(None);
        d.key = key.into();
        d
    }
    #[test]
    fn identity_cycle_close_and_empty() {
        let mut t = Tabs::default();
        t.open(tab("a"));
        t.open(tab("b"));
        t.open(tab("a"));
        assert_eq!(t.items.len(), 2);
        t.cycle(false);
        assert_eq!(t.active.as_deref(), Some("b"));
        t.close("a");
        assert_eq!(t.active.as_deref(), Some("b"));
        t.close("b");
        assert!(t.current().is_none());
        t.cycle(true);
    }
    #[test]
    fn successor_reorder_and_bulk_close() {
        let mut t = Tabs::default();
        for key in ["a", "b", "c", "d"] {
            t.open(tab(key));
        }
        t.move_tab("d", -1);
        assert_eq!(t.items[2].key, "d");
        t.select("b");
        t.close("b");
        assert_eq!(t.active.as_deref(), Some("d"));
        t.retain("d", true);
        assert_eq!(t.items.len(), 2);
        t.retain("d", false);
        assert_eq!(t.items.len(), 1);
    }
    #[test]
    fn closing_the_active_tab_does_not_leave_a_noop_history_button() {
        let mut t = Tabs::default();
        for key in ["a", "b", "c"] {
            t.open(tab(key));
        }
        t.close("c");
        t.travel(false);
        assert_eq!(t.active.as_deref(), Some("a"));
        assert!(!t.can_back());
        assert!(t.can_forward());
        t.close("a");
        assert_eq!(t.active.as_deref(), Some("b"));
        assert!(!t.can_forward());
        t.close_all();
        assert!(t.current().is_none());
        assert_eq!(t.closed.len(), 3);
        t.close("missing");
        assert_eq!(t.closed.len(), 3);
    }
    #[test]
    fn history_skips_closed_and_branches() {
        let mut t = Tabs::default();
        for key in ["a", "b", "c"] {
            t.open(tab(key));
        }
        t.travel(false);
        assert_eq!(t.active.as_deref(), Some("b"));
        t.travel(true);
        assert_eq!(t.active.as_deref(), Some("c"));
        t.close("b");
        t.travel(false);
        assert_eq!(t.active.as_deref(), Some("a"));
        t.open(tab("d"));
        assert!(!t.can_forward());
    }
}
