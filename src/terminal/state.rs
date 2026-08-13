#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TerminalGrid {
    pub rows: u16,
    pub cols: u16,
}

pub struct TerminalState {
    parser: vt100::Parser,
    grid: TerminalGrid,
}

impl TerminalState {
    pub fn new(grid: TerminalGrid, scrollback_lines: usize) -> Self {
        Self {
            parser: vt100::Parser::new(grid.rows, grid.cols, scrollback_lines),
            grid,
        }
    }

    pub fn process(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    pub fn resize(&mut self, grid: TerminalGrid) {
        self.grid = grid;
        self.parser.screen_mut().set_size(grid.rows, grid.cols);
    }

    pub fn set_scrollback(&mut self, rows: usize) {
        self.parser.screen_mut().set_scrollback(rows);
    }

    pub fn visible_rows(&self) -> Vec<String> {
        self.parser.screen().rows(0, self.grid.cols).collect()
    }

    pub fn cursor_position(&self) -> (u16, u16) {
        let screen = self.parser.screen();
        let (rows, cols) = screen.size();
        let row = screen.cursor_position().0.min(rows.saturating_sub(1));
        let col = screen.cursor_position().1.min(cols.saturating_sub(1));
        (row, col)
    }

    /// Whether the terminal is currently scrolled back so the live screen is
    /// not in view.
    pub fn scrollback(&self) -> usize {
        self.parser.screen().scrollback()
    }

    /// Whether the terminal emitted a "hide cursor" escape sequence
    /// (DECTCEM), e.g. full-screen applications.
    pub fn cursor_hidden(&self) -> bool {
        self.parser.screen().hide_cursor()
    }

    pub fn cell(&self, row: u16, col: u16) -> Option<&vt100::Cell> {
        self.parser.screen().cell(row, col)
    }
}
