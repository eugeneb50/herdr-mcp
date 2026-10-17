//! Navigation primitives for the TUI dashboard.
//!
//! `NavigationFrame` wraps a selection index with bounds clamping.
//! `FrameFocus` tracks which frame within a tab has keyboard focus.

/// A scrollable list selection with bounds clamping.
/// Replaces raw `usize` index fields across all tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NavigationFrame {
    /// Current selection index.
    pub index: usize,
}

impl NavigationFrame {
    pub fn new() -> Self {
        Self::default()
    }

    /// Move up one item. No-op if already at top.
    pub fn up(&mut self, len: usize) {
        self.index = self.index.saturating_sub(1).min(len.saturating_sub(1));
    }

    /// Move down one item. No-op if already at bottom.
    pub fn down(&mut self, len: usize) {
        if len == 0 {
            self.index = 0;
            return;
        }
        self.index = (self.index + 1).min(len - 1);
    }

    /// Jump to first item.
    pub fn top(&mut self) {
        self.index = 0;
    }

    /// Jump to last item.
    pub fn bottom(&mut self, len: usize) {
        self.index = len.saturating_sub(1);
    }

    /// Fix index if the list shrank (call before every render).
    pub fn clamp(&mut self, len: usize) {
        if len == 0 {
            self.index = 0;
        } else if self.index >= len {
            self.index = len - 1;
        }
    }

    /// Page up by `page` rows.
    pub fn page_up(&mut self, page: usize) {
        self.index = self.index.saturating_sub(page);
    }

    /// Page down by `page` rows.
    pub fn page_down(&mut self, len: usize, page: usize) {
        if len == 0 {
            self.index = 0;
            return;
        }
        self.index = (self.index + page).min(len - 1);
    }

    /// Scroll down (same as down — clamps at bottom).
    pub fn scroll_down(&mut self, len: usize) {
        self.down(len);
    }

    /// Scroll up (same as up — clamps at top).
    pub fn scroll_up(&mut self, len: usize) {
        self.up(len);
    }
}

/// Tracks which frame (panel/section) within a tab has keyboard focus.
/// Global Tab/BackTab cycles this; the tab-specific handler uses it
/// to route arrow keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameFocus {
    /// Index of the currently focused frame.
    pub active: usize,
    /// Total number of focusable frames on this tab.
    pub count: usize,
}

impl FrameFocus {
    pub fn new(count: usize) -> Self {
        Self { active: 0, count }
    }

    /// Cycle to the next frame (Tab).
    pub fn next(&mut self) {
        if self.count > 1 {
            self.active = (self.active + 1) % self.count;
        }
    }

    /// Cycle to the previous frame (Shift+Tab).
    pub fn prev(&mut self) {
        if self.count > 1 {
            self.active = (self.active + self.count - 1) % self.count;
        }
    }

    /// Check if a specific frame index is focused.
    pub fn is_active(&self, frame: usize) -> bool {
        self.active == frame
    }

    /// Reset focus to frame 0 (e.g., when switching tabs).
    pub fn reset(&mut self) {
        self.active = 0;
    }
}

/// Cycle a frame focus index forward (step > 0) or backward (step < 0).
pub fn cycle_frame(focused: &mut usize, count: usize, step: i32) {
    if count <= 1 {
        return;
    }
    *focused = ((*focused as i32 + step + count as i32) % count as i32) as usize;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nav_frame_up_down() {
        let mut f = NavigationFrame::new();
        f.down(5);
        assert_eq!(f.index, 1);
        f.down(5);
        assert_eq!(f.index, 2);
        f.up(5);
        assert_eq!(f.index, 1);
    }

    #[test]
    fn nav_frame_clamps_at_top() {
        let mut f = NavigationFrame { index: 0 };
        f.up(5);
        assert_eq!(f.index, 0);
    }

    #[test]
    fn nav_frame_clamps_at_bottom() {
        let mut f = NavigationFrame { index: 4 };
        f.down(5);
        assert_eq!(f.index, 4);
    }

    #[test]
    fn nav_frame_empty_list() {
        let mut f = NavigationFrame::new();
        f.down(0);
        assert_eq!(f.index, 0);
        f.up(0);
        assert_eq!(f.index, 0);
    }

    #[test]
    fn nav_frame_top_bottom() {
        let mut f = NavigationFrame { index: 3 };
        f.top();
        assert_eq!(f.index, 0);
        f.bottom(5);
        assert_eq!(f.index, 4);
    }

    #[test]
    fn nav_frame_page_up_down() {
        let mut f = NavigationFrame { index: 10 };
        f.page_up(5);
        assert_eq!(f.index, 5);
        f.page_down(20, 5);
        assert_eq!(f.index, 10);
    }

    #[test]
    fn nav_frame_clamp() {
        let mut f = NavigationFrame { index: 10 };
        f.clamp(5);
        assert_eq!(f.index, 4);

        let mut f2 = NavigationFrame { index: 3 };
        f2.clamp(0);
        assert_eq!(f2.index, 0);
    }

    #[test]
    fn frame_focus_cycles() {
        let mut ff = FrameFocus::new(3);
        assert_eq!(ff.active, 0);
        ff.next();
        assert_eq!(ff.active, 1);
        ff.next();
        assert_eq!(ff.active, 2);
        ff.next(); // wraps
        assert_eq!(ff.active, 0);
        ff.prev(); // wraps backward
        assert_eq!(ff.active, 2);
    }

    #[test]
    fn frame_focus_noop_when_single() {
        let mut ff = FrameFocus::new(1);
        ff.next();
        assert_eq!(ff.active, 0);
        ff.prev();
        assert_eq!(ff.active, 0);
    }

    #[test]
    fn cycle_frame_fn() {
        let mut focused = 0usize;
        cycle_frame(&mut focused, 3, 1);
        assert_eq!(focused, 1);
        cycle_frame(&mut focused, 3, 1);
        assert_eq!(focused, 2);
        cycle_frame(&mut focused, 3, 1); // wraps
        assert_eq!(focused, 0);
        cycle_frame(&mut focused, 3, -1); // wraps backward
        assert_eq!(focused, 2);
    }

    #[test]
    fn cycle_frame_noop_when_single() {
        let mut focused = 0usize;
        cycle_frame(&mut focused, 1, 1);
        assert_eq!(focused, 0);
    }
}
