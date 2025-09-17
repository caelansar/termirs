use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, List, ListItem, Scrollbar, ScrollbarOrientation, ScrollbarState,
};

#[derive(Clone, Debug)]
pub struct DropdownState {
    pub options: Vec<String>,
    pub selected: usize,
    pub visible: bool,
    pub scroll_offset: usize,            // Track the scroll position
    pub max_visible_items: usize,        // Maximum items to show at once
    pub scrollbar_state: ScrollbarState, // State for the scrollbar widget
}

impl DropdownState {
    pub fn new(options: Vec<String>) -> Self {
        let content_length = options.len();
        Self {
            options,
            selected: 0,
            visible: true,
            scroll_offset: 0,
            max_visible_items: 8, // Default to 8 visible items
            scrollbar_state: ScrollbarState::new(content_length).position(0),
        }
    }

    pub fn next(&mut self) {
        if !self.options.is_empty() {
            self.selected = (self.selected + 1) % self.options.len();
            self.update_scroll();
        }
    }

    pub fn prev(&mut self) {
        if !self.options.is_empty() {
            self.selected = if self.selected == 0 {
                self.options.len() - 1
            } else {
                self.selected - 1
            };
            self.update_scroll();
        }
    }

    /// Update scroll offset to keep selected item visible
    fn update_scroll(&mut self) {
        if self.options.is_empty() {
            return;
        }

        // If selected item is above the visible window, scroll up
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        }
        // If selected item is below the visible window, scroll down
        else if self.selected >= self.scroll_offset + self.max_visible_items {
            self.scroll_offset = self.selected.saturating_sub(self.max_visible_items - 1);
        }

        // Update scrollbar state to reflect current position
        self.scrollbar_state = self.scrollbar_state.position(self.selected);
    }

    pub fn get_selected(&self) -> Option<&String> {
        self.options.get(self.selected)
    }
}

pub fn draw_dropdown_with_rect(
    dropdown: &mut DropdownState,
    anchor_rect: Rect,
    frame: &mut ratatui::Frame<'_>,
) {
    if !dropdown.visible || dropdown.options.is_empty() {
        return;
    }

    // Calculate dropdown position and size
    let visible_items = dropdown.options.len().min(dropdown.max_visible_items);
    let dropdown_height = visible_items as u16 + 2; // +2 for borders

    // Position dropdown below the anchor field
    let x = anchor_rect.x;
    let y = anchor_rect.y + anchor_rect.height;
    let width = anchor_rect.width;

    let dropdown_rect = Rect {
        x,
        y,
        width,
        height: dropdown_height,
    };

    // Clear the area first
    frame.render_widget(Clear, dropdown_rect);

    // Split the dropdown area to make room for scrollbar if needed
    let show_scrollbar = dropdown.options.len() > dropdown.max_visible_items;
    let (list_area, scrollbar_area) = if show_scrollbar {
        // Get the inner area (inside borders) first
        let inner_area = Rect {
            x: dropdown_rect.x + 1,
            y: dropdown_rect.y + 1,
            width: dropdown_rect.width.saturating_sub(2),
            height: dropdown_rect.height.saturating_sub(2),
        };

        // Reserve 1 column for scrollbar on the right inside the borders
        let list_area = Rect {
            x: dropdown_rect.x,
            y: dropdown_rect.y,
            width: dropdown_rect.width.saturating_sub(1), // Make room for scrollbar
            height: dropdown_rect.height,
        };

        let scrollbar_area = Rect {
            x: inner_area.x + inner_area.width.saturating_sub(1), // Position inside right border
            y: inner_area.y,
            width: 1,
            height: inner_area.height,
        };
        (list_area, Some(scrollbar_area))
    } else {
        (dropdown_rect, None)
    };

    // Get the visible slice of options based on scroll offset
    let end_index =
        (dropdown.scroll_offset + dropdown.max_visible_items).min(dropdown.options.len());
    let visible_options = &dropdown.options[dropdown.scroll_offset..end_index];

    // Create list items for visible options only
    let list_items: Vec<ListItem> = visible_options
        .iter()
        .enumerate()
        .map(|(visible_index, option)| {
            let actual_index = dropdown.scroll_offset + visible_index;
            let style = if actual_index == dropdown.selected {
                Style::default()
                    .bg(Color::Cyan)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            ListItem::new(Line::from(Span::styled(option.clone(), style)))
        })
        .collect();

    // Create title - simpler now since we have visual scrollbar
    let title = format!(
        "Options ({}/{})",
        dropdown.selected + 1,
        dropdown.options.len()
    );

    // Create the list widget
    let list = List::new(list_items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(title),
    );

    frame.render_widget(list, list_area);

    // Render scrollbar if needed
    if let Some(scrollbar_area) = scrollbar_area {
        let scrollbar = Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None) // Remove symbols to fit better inside borders
            .end_symbol(None)
            .track_symbol(Some("│"))
            .thumb_symbol("█")
            .style(Style::default().fg(Color::Cyan));

        frame.render_stateful_widget(scrollbar, scrollbar_area, &mut dropdown.scrollbar_state);
    }
}
