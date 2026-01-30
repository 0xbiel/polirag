use std::io::{self, Stdout};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Alignment},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, List, ListItem, ListState, Wrap},
    Frame, Terminal,
};
use tokio::sync::mpsc;
use futures::StreamExt;

use crate::llm::ChatMessage;
use crate::AppState;
use crate::rag::RagStats;

mod markdown;

const THROBBER_FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

#[derive(PartialEq, Clone)]
pub enum AppMode {
    Menu,
    Chat,
    RagInfo,
    Login,
    Sync,
    Settings,
}

pub struct TuiApp {
    pub mode: AppMode,
    // Menu State
    pub menu_items: Vec<String>,
    pub menu_state: ListState,
    pub is_connected: bool,
    
    // Chat State
    pub messages: Vec<ChatMessage>,
    pub input: String,
    pub input_cursor: usize,
    pub scroll_offset: u16,
    pub follow_bottom: bool,
    pub is_thinking: bool,
    pub throbber_frame: usize,
    pub model_name: String,
    
    // RAG Info
    pub rag_stats: Option<RagStats>,
    
    // Login State
    pub login_username: String,
    pub login_pin: String,
    pub login_field: usize,
    pub login_error: Option<String>,
    
    // Sync State
    pub sync_logs: Vec<String>,
    pub sync_running: bool,
    pub sync_complete: bool,
    
    // Settings State
    pub available_models: Vec<String>,
    pub model_state: ListState,
    pub models_loading: bool,
    pub active_provider: crate::config::LlmProvider,
    pub settings_input_mode: bool, // false = navigating, true = editing
    pub settings_field: usize, // 0=Provider, 1=Model List/Input, 2=API Key
    pub openrouter_key: String,
    pub openrouter_model: String,
    
    // Global
    pub should_quit: bool,
    pub content_height: u16,
    pub viewport_height: u16,
    pub status_message: Option<String>,
    pub status_message_time: Option<Instant>,
    pub context_limit: usize,
    pub last_request_tokens: usize,
    
    // Reembed State
    pub reembed_running: bool,
    pub reembed_progress: String,
}

impl TuiApp {
    pub fn new(model_name: String, connected: bool) -> Self {
        let config = crate::config::Config::load();
        let mut menu_state = ListState::default();
        menu_state.select(Some(0));
        
        Self {
            mode: AppMode::Menu,
            menu_items: vec![
                "💬 Chat with Assistant".to_string(),
                "🔄 Sync Data".to_string(),
                "📊 View RAG Index Info".to_string(),
                "🔐 Login to PoliformaT".to_string(),
                "⚙️  Settings (Model)".to_string(),
                "🚪 Exit".to_string()
            ],
            menu_state,
            is_connected: connected,
            
            messages: vec![
                ChatMessage {
                    role: "system".to_string(),
                    content: "You are a helpful assistant with access to the user's university documents (PoliformaT). Use the provided context to answer questions. IMPORTANT: You MUST answer in the same language as the user's message (e.g. if user asks in Catalan, answer in Catalan; if in English, answer in English), even if the retrieved documents are in Spanish.".to_string(),
                    thinking_collapsed: false,
                }
            ],
            input: String::new(),
            input_cursor: 0,
            scroll_offset: 0,
            follow_bottom: true,
            is_thinking: false,
            throbber_frame: 0,
            model_name,
            
            rag_stats: None,
            
            login_username: String::new(),
            login_pin: String::new(),
            login_field: 0,
            login_error: None,
            
            sync_logs: Vec::new(),
            sync_running: false,
            sync_complete: false,
            
            available_models: Vec::new(),
            model_state: ListState::default(),
            models_loading: false,
            
            active_provider: config.llm_provider,
            settings_input_mode: false,
            settings_field: 0,
            openrouter_key: config.openrouter_api_key.unwrap_or_default(),
            openrouter_model: config.openrouter_model.unwrap_or_default(),
            
            should_quit: false,
            content_height: 0,
            viewport_height: 0,
            status_message: None,
            status_message_time: None,
            context_limit: 32768,
            last_request_tokens: 0,
            
            reembed_running: false,
            reembed_progress: String::new(),
        }
    }

    pub fn scroll_up(&mut self, amount: u16) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
        self.follow_bottom = false;
    }

    pub fn scroll_down(&mut self, amount: u16) {
        let max_scroll = self.content_height.saturating_sub(self.viewport_height);
        self.scroll_offset = (self.scroll_offset + amount).min(max_scroll);
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = self.content_height.saturating_sub(self.viewport_height);
        self.follow_bottom = true;
    }

    pub fn advance_throbber(&mut self) {
        self.throbber_frame = (self.throbber_frame + 1) % THROBBER_FRAMES.len();
    }
    
    pub fn next_menu_item(&mut self) {
        let i = match self.menu_state.selected() {
            Some(i) => if i >= self.menu_items.len() - 1 { 0 } else { i + 1 },
            None => 0,
        };
        self.menu_state.select(Some(i));
    }

    pub fn previous_menu_item(&mut self) {
        let i = match self.menu_state.selected() {
            Some(i) => if i == 0 { self.menu_items.len() - 1 } else { i - 1 },
            None => 0,
        };
        self.menu_state.select(Some(i));
    }
    
    pub fn next_model(&mut self) {
        if self.available_models.is_empty() { return; }
        let i = match self.model_state.selected() {
            Some(i) => if i >= self.available_models.len() - 1 { 0 } else { i + 1 },
            None => 0,
        };
        self.model_state.select(Some(i));
    }

    pub fn previous_model(&mut self) {
        if self.available_models.is_empty() { return; }
        let i = match self.model_state.selected() {
            Some(i) => if i == 0 { self.available_models.len() - 1 } else { i - 1 },
            None => 0,
        };
        self.model_state.select(Some(i));
    }
    
    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status_message = Some(msg.into());
        self.status_message_time = Some(Instant::now());
    }
}

pub fn setup_terminal() -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend)
}

pub fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

// ============================================================================
// DRAWING FUNCTIONS
// ============================================================================

fn draw(frame: &mut Frame, app: &mut TuiApp) {
    match app.mode {
        AppMode::Menu => draw_menu(frame, app),
        AppMode::Chat => draw_chat(frame, app),
        AppMode::RagInfo => draw_rag_info(frame, app),
        AppMode::Login => draw_login(frame, app),
        AppMode::Sync => draw_sync(frame, app),
        AppMode::Settings => draw_settings(frame, app),
    }
}

fn render_logo() -> Vec<Line<'static>> {
    vec![
        Line::from(Span::styled("██████╗  ██████╗ ██╗     ██╗██████╗  █████╗  ██████╗ ", Style::default().fg(Color::Cyan))),
        Line::from(Span::styled("██╔══██╗██╔═══██╗██║     ██║██╔══██╗██╔══██╗██╔════╝ ", Style::default().fg(Color::Cyan))),
        Line::from(Span::styled("██████╔╝██║   ██║██║     ██║██████╔╝███████║██║  ███╗", Style::default().fg(Color::Cyan))),
        Line::from(Span::styled("██╔═══╝ ██║   ██║██║     ██║██╔══██╗██╔══██║██║   ██║", Style::default().fg(Color::Cyan))),
        Line::from(Span::styled("██║     ╚██████╔╝███████╗██║██║  ██║██║  ██║╚██████╔╝", Style::default().fg(Color::Cyan))),
        Line::from(Span::styled("╚═╝      ╚═════╝ ╚══════╝╚═╝╚═╝  ╚═╝╚═╝  ╚═╝ ╚═════╝ ", Style::default().fg(Color::Cyan))),
    ]
}

fn draw_menu(frame: &mut Frame, app: &mut TuiApp) {
    let size = frame.area();
    
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" PoliRag ");
        
    let inner_area = block.inner(size);
    frame.render_widget(block, size);
    
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(7),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(5),
            Constraint::Length(2),
        ])
        .margin(1)
        .split(inner_area);
        
    let logo = Paragraph::new(render_logo()).alignment(Alignment::Center);
    frame.render_widget(logo, layout[0]);
    
    let status_str = if app.is_connected { "● Connected to PoliformaT" } else { "○ Disconnected" };
    let status_color = if app.is_connected { Color::Green } else { Color::Red };
    let status = Paragraph::new(Span::styled(status_str, Style::default().fg(status_color).add_modifier(Modifier::BOLD)))
        .alignment(Alignment::Center);
    frame.render_widget(status, layout[2]);
    
    let items: Vec<ListItem> = app.menu_items
        .iter()
        .map(|i| ListItem::new(Line::from(format!("  {}", i))))
        .collect();
        
    let menu = List::new(items)
        .highlight_style(Style::default().bg(Color::Cyan).fg(Color::Black).add_modifier(Modifier::BOLD))
        .highlight_symbol(" ▶ ");
        
    let menu_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(25), Constraint::Percentage(50), Constraint::Percentage(25)])
        .split(layout[4]);
        
    frame.render_stateful_widget(menu, menu_layout[1], &mut app.menu_state);
    
    let instr = Paragraph::new("↑/↓ Navigate  │  Enter Select  │  Esc Exit")
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center);
    frame.render_widget(instr, layout[5]);
}

fn draw_chat(frame: &mut Frame, app: &mut TuiApp) {
    let size = frame.area();
    
    let outer_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(format!(" PoliRag Chat │ {} ", app.model_name))
        .title_bottom(Line::from(format!(" {}/{} tokens ", app.last_request_tokens, app.context_limit)).right_aligned());
    
    let inner_area = outer_block.inner(size);
    frame.render_widget(outer_block, size);
    
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),
            Constraint::Length(1),
            Constraint::Length(3),
        ])
        .split(inner_area);

    let messages_area = chunks[0];
    app.viewport_height = messages_area.height;
    
    let max_width = messages_area.width.saturating_sub(4) as usize;
    let mut lines: Vec<Line> = Vec::new();
    
    for msg in &app.messages {
        match msg.role.as_str() {
            "user" => {
                lines.push(Line::from(""));
                lines.push(Line::from(vec![
                    Span::styled(" ▶ You ", Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD)),
                ]));
                // Users messages are usually simple, but we can markdown them too
                let rendered = markdown::render_markdown(&msg.content, max_width, false);
                lines.extend(rendered);
            }
            "assistant" => {
                lines.push(Line::from(""));
                lines.push(Line::from(vec![
                    Span::styled(" ◆ Assistant ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                ]));
                let rendered = markdown::render_markdown(&msg.content, max_width, msg.thinking_collapsed);
                lines.extend(rendered);
            }
            _ => {}
        }
    }

    if app.is_thinking {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {} Thinking...", THROBBER_FRAMES[app.throbber_frame]),
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
        ]));
    }

    // Estimate content height based on wrapping
    // content_height = sum of visual lines
    let mut total_height = 0;
    for line in &lines {
        // Reconstruct string to measure wrapping (styles don't affect wrapping usually)
        let mut full_line_str = String::new();
        for span in &line.spans {
            full_line_str.push_str(&span.content);
        }
        
        let wrapped_lines = textwrap::wrap(&full_line_str, max_width);
        // Ensure at least 1 line for empty strings? textwrap returns empty vec for empty string.
        let output_lines = wrapped_lines.len().max(1);
        total_height += output_lines;
    }
    app.content_height = total_height as u16;

    let max_scroll = app.content_height.saturating_sub(app.viewport_height);
    if app.follow_bottom { app.scroll_offset = max_scroll; }
    else if app.scroll_offset > max_scroll { app.scroll_offset = max_scroll; }

    let messages = Paragraph::new(Text::from(lines))
        .wrap(Wrap { trim: false })
        .scroll((app.scroll_offset, 0));
    frame.render_widget(messages, messages_area);

    if app.content_height > app.viewport_height {
        let scrollbar = Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .thumb_style(Style::default().fg(Color::Cyan))
            .track_style(Style::default().fg(Color::DarkGray));
        let mut scrollbar_state = ScrollbarState::new(app.content_height as usize)
            .position(app.scroll_offset as usize)
            .viewport_content_length(app.viewport_height as usize);
        frame.render_stateful_widget(scrollbar, messages_area, &mut scrollbar_state);
    }

    let status_text = app.status_message.clone().unwrap_or_else(|| "Esc Menu │ Ctrl+L Clear │ /model <name>".to_string());
    let status = Paragraph::new(status_text).style(Style::default().fg(Color::DarkGray)).alignment(Alignment::Center);
    frame.render_widget(status, chunks[1]);

    let input_block = Block::default()
        .borders(Borders::TOP)
        .border_style(if app.is_thinking { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::Cyan) })
        .title(" Message ");
    let input_text = Paragraph::new(app.input.as_str()).block(input_block).style(Style::default().fg(Color::White));
    frame.render_widget(input_text, chunks[2]);

    if !app.is_thinking {
        let cursor_x = chunks[2].x + app.input_cursor as u16;
        let cursor_y = chunks[2].y + 1;
        frame.set_cursor_position((cursor_x.min(chunks[2].x + chunks[2].width - 1), cursor_y));
    }
}

fn draw_rag_info(frame: &mut Frame, app: &mut TuiApp) {
    let size = frame.area();
    
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" RAG Index Information ");
    let inner_area = block.inner(size);
    frame.render_widget(block, size);
    
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Length(1), Constraint::Min(8), Constraint::Length(3), Constraint::Length(2)])
        .margin(1)
        .split(inner_area);
    
    let logo = Paragraph::new(render_logo()).alignment(Alignment::Center);
    frame.render_widget(logo, layout[0]);
    
    let content = if let Some(stats) = &app.rag_stats {
        let mut lines = vec![
            Line::from(""),
            Line::from(vec![Span::styled("  📁 Storage Path:    ", Style::default().add_modifier(Modifier::BOLD)), Span::raw(&stats.storage_path)]),
            Line::from(vec![Span::styled("  🗄️  Store Type:      ", Style::default().add_modifier(Modifier::BOLD)), Span::styled(&stats.store_type, Style::default().fg(Color::Cyan))]),
            Line::from(vec![Span::styled("  ✂️  Chunking:        ", Style::default().add_modifier(Modifier::BOLD)), Span::raw(&stats.chunking_strategy)]),
            Line::from(vec![Span::styled("  🧠 Embedding Model: ", Style::default().add_modifier(Modifier::BOLD)), Span::raw(&stats.embedding_model)]),
            Line::from(vec![Span::styled("  💾 Index Size:      ", Style::default().add_modifier(Modifier::BOLD)), Span::styled(stats.format_file_size(), Style::default().fg(Color::Green))]),
            Line::from(vec![Span::styled("  📄 Documents:       ", Style::default().add_modifier(Modifier::BOLD)), Span::styled(stats.document_count.to_string(), Style::default().fg(Color::Yellow))]),
            Line::from(vec![Span::styled("  📝 Content Size:    ", Style::default().add_modifier(Modifier::BOLD)), Span::raw(stats.format_content_size())]),
            Line::from(""),
            Line::from(Span::styled("  Documents by Type:", Style::default().add_modifier(Modifier::BOLD).add_modifier(Modifier::UNDERLINED))),
        ];
        for (t, c) in &stats.docs_by_type {
            lines.push(Line::from(format!("    • {}: {}", t, c)));
        }
        lines
    } else {
        vec![Line::from(""), Line::from(Span::styled("  ⏳ Loading...", Style::default().fg(Color::Yellow)))]
    };
    frame.render_widget(Paragraph::new(content), layout[2]);
    
    // Action button area
    let button_area = layout[3];
    if app.reembed_running {
        let progress = Paragraph::new(format!("{} {}", THROBBER_FRAMES[app.throbber_frame], app.reembed_progress))
            .style(Style::default().fg(Color::Yellow))
            .alignment(Alignment::Center);
        frame.render_widget(progress, button_area);
    } else {
        let button = Paragraph::new("  ▶ [R] Recalculate Embeddings  ")
            .style(Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD))
            .alignment(Alignment::Center);
        frame.render_widget(button, button_area);
    }
    
    let instr_text = if app.reembed_running { 
        "Recalculating embeddings..." 
    } else { 
        "R Recalculate │ Esc Menu" 
    };
    let instr = Paragraph::new(instr_text).style(Style::default().fg(Color::DarkGray)).alignment(Alignment::Center);
    frame.render_widget(instr, layout[4]);
}

fn draw_login(frame: &mut Frame, app: &mut TuiApp) {
    let size = frame.area();
    
    let block = Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::Cyan)).title(" Login to PoliformaT ");
    let inner_area = block.inner(size);
    frame.render_widget(block, size);
    
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Length(2), Constraint::Length(3), Constraint::Length(1), Constraint::Length(3), Constraint::Length(2), Constraint::Min(2), Constraint::Length(2)])
        .margin(1)
        .split(inner_area);
    
    frame.render_widget(Paragraph::new(render_logo()).alignment(Alignment::Center), layout[0]);
    
    let form_layout = Layout::default().direction(Direction::Horizontal).constraints([Constraint::Percentage(25), Constraint::Percentage(50), Constraint::Percentage(25)]).split(layout[2]);
    let form_layout_pin = Layout::default().direction(Direction::Horizontal).constraints([Constraint::Percentage(25), Constraint::Percentage(50), Constraint::Percentage(25)]).split(layout[4]);
    
    let username_style = if app.login_field == 0 { Style::default().fg(Color::Cyan) } else { Style::default().fg(Color::DarkGray) };
    let username_block = Block::default().borders(Borders::ALL).border_style(username_style).title(" Username/DNI ");
    frame.render_widget(Paragraph::new(app.login_username.as_str()).block(username_block), form_layout[1]);
    
    let pin_style = if app.login_field == 1 { Style::default().fg(Color::Cyan) } else { Style::default().fg(Color::DarkGray) };
    let pin_block = Block::default().borders(Borders::ALL).border_style(pin_style).title(" PIN/Password ");
    frame.render_widget(Paragraph::new("*".repeat(app.login_pin.len())).block(pin_block), form_layout_pin[1]);
    
    if let Some(error) = &app.login_error {
        frame.render_widget(Paragraph::new(error.as_str()).style(Style::default().fg(Color::Red)).alignment(Alignment::Center), layout[5]);
    } else if app.is_thinking {
        frame.render_widget(Paragraph::new(format!("{} Logging in...", THROBBER_FRAMES[app.throbber_frame])).style(Style::default().fg(Color::Yellow)).alignment(Alignment::Center), layout[5]);
    }
    
    if !app.is_thinking {
        let (cursor_x, cursor_y) = if app.login_field == 0 {
            (form_layout[1].x + app.login_username.len() as u16 + 1, form_layout[1].y + 1)
        } else {
            (form_layout_pin[1].x + app.login_pin.len() as u16 + 1, form_layout_pin[1].y + 1)
        };
        frame.set_cursor_position((cursor_x, cursor_y));
    }
    
    frame.render_widget(Paragraph::new("Tab Switch Field │ Enter Submit │ Esc Cancel").style(Style::default().fg(Color::DarkGray)).alignment(Alignment::Center), layout[7]);
}

fn draw_sync(frame: &mut Frame, app: &mut TuiApp) {
    let size = frame.area();
    
    let title = if app.sync_running {
        format!(" Syncing... {} ", THROBBER_FRAMES[app.throbber_frame])
    } else if app.sync_complete {
        " Sync Complete ✓ ".to_string()
    } else {
        " Sync Data ".to_string()
    };
    
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(if app.sync_complete { Style::default().fg(Color::Green) } else { Style::default().fg(Color::Cyan) })
        .title(title);
    let inner_area = block.inner(size);
    frame.render_widget(block, size);
    
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Length(1), Constraint::Min(5), Constraint::Length(2)])
        .margin(1)
        .split(inner_area);
    
    frame.render_widget(Paragraph::new(render_logo()).alignment(Alignment::Center), layout[0]);
    
    // Log area
    let log_area = layout[2];
    app.viewport_height = log_area.height;
    
    let log_lines: Vec<Line> = app.sync_logs.iter().map(|log| {
        let color = if log.contains("Error") || log.contains("Failed") {
            Color::Red
        } else if log.contains("Complete") || log.contains("Success") {
            Color::Green
        } else if log.contains("...") {
            Color::Yellow
        } else {
            Color::White
        };
        Line::from(Span::styled(format!(" {} ", log), Style::default().fg(color)))
    }).collect();
    
    app.content_height = log_lines.len() as u16;
    let max_scroll = app.content_height.saturating_sub(app.viewport_height);
    if app.follow_bottom { app.scroll_offset = max_scroll; }
    
    let logs = Paragraph::new(log_lines)
        .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::DarkGray)).title(" Logs "))
        .scroll((app.scroll_offset, 0));
    frame.render_widget(logs, log_area);
    
    let instr_text = if app.sync_running { "Syncing in progress..." } else { "Press Esc to return to Menu" };
    frame.render_widget(Paragraph::new(instr_text).style(Style::default().fg(Color::DarkGray)).alignment(Alignment::Center), layout[3]);
}

fn _draw_settings_old(frame: &mut Frame, app: &mut TuiApp) {
    let size = frame.area();
    
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" Settings ");
    let inner_area = block.inner(size);
    frame.render_widget(block, size);
    
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(6), // Logo
            Constraint::Length(3), // Provider Select
            Constraint::Length(3), // Input 1 (Model List or API Key)
            Constraint::Length(3), // Input 2 (Model Name)
            Constraint::Min(3),    // Remaining/Help
        ])
        .margin(1)
        .split(inner_area);
    
    frame.render_widget(Paragraph::new(render_logo()).alignment(Alignment::Center), layout[0]);
    
    // Current model
    let current = Paragraph::new(format!("Current Model: {}", app.model_name))
        .style(Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center);
    frame.render_widget(current, layout[2]);
    
    // Model list
    if app.models_loading {
        frame.render_widget(
            Paragraph::new(format!("{} Loading models...", THROBBER_FRAMES[app.throbber_frame]))
                .style(Style::default().fg(Color::Yellow))
                .alignment(Alignment::Center),
            layout[3]
        );
    } else if app.available_models.is_empty() {
        frame.render_widget(
            Paragraph::new("No models found. Is your LLM server running?")
                .style(Style::default().fg(Color::Red))
                .alignment(Alignment::Center),
            layout[3]
        );
    } else {
        let items: Vec<ListItem> = app.available_models.iter().map(|m| {
            let style = if m == &app.model_name {
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(Span::styled(format!("  {}", m), style)))
        }).collect();
        
        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::DarkGray)).title(" Available Models "))
            .highlight_style(Style::default().bg(Color::Cyan).fg(Color::Black).add_modifier(Modifier::BOLD))
            .highlight_symbol(" ▶ ");
        
        let model_layout = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(15), Constraint::Percentage(70), Constraint::Percentage(15)])
            .split(layout[3]);
        
        frame.render_stateful_widget(list, model_layout[1], &mut app.model_state);
    }
    
    frame.render_widget(
        Paragraph::new("↑/↓ Navigate  │  Enter Select  │  Esc Back")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center),
        layout[4]
    );
}

// function moved to markdown.rs

// ============================================================================
// ASYNC MESSAGING
// ============================================================================

enum LlmResult {
    StreamChunk(crate::llm::StreamEvent),
    StreamDone,
    Error(String),
    ModelList(Vec<String>),
}

enum SyncResult {
    Success,
    Error(String),
    Log(String),
}

enum LoginResult {
    Success,
    Error(String),
}

enum ReembedResult {
    Progress(String),
    Complete(usize),
    Error(String),
}

// ============================================================================
// MAIN APP LOOP
// ============================================================================

pub async fn run_app(state: Arc<AppState>) -> anyhow::Result<()> {
    // Load config to set initial LLM state
    let config = crate::config::Config::load();
    {
        let mut llm = state.llm.lock().unwrap();
        llm.set_auth(config.llm_provider.base_url(), config.openrouter_api_key.clone());
        if let Some(model) = &config.openrouter_model {
            if config.llm_provider == crate::config::LlmProvider::OpenRouter {
                llm.set_model(model);
            }
        }
    }

    let connected = state.poliformat.check_connection().await.unwrap_or(false);
    let model_name = state.llm.lock().unwrap().model.clone();
    
    let mut app = TuiApp::new(model_name, connected);
    
    // Fetch context limit from API
    if let Ok(ctx_len) = state.llm.lock().unwrap().fetch_context_length().await {
        app.context_limit = ctx_len;
    }
    
    let mut terminal = setup_terminal()?;
    
    let tick_rate = Duration::from_millis(80);
    let mut last_tick = Instant::now();
    
    let (tx_llm, mut rx_llm) = mpsc::channel::<LlmResult>(10);
    let (tx_sync, mut rx_sync) = mpsc::channel::<SyncResult>(100);
    let (tx_login, mut rx_login) = mpsc::channel::<LoginResult>(1);
    let (tx_reembed, mut rx_reembed) = mpsc::channel::<ReembedResult>(100);

    loop {
        terminal.draw(|f| draw(f, &mut app))?;

        // Check LLM results
        if let Ok(result) = rx_llm.try_recv() {
            match result {
                LlmResult::StreamChunk(event) => {
                    match event {
                        crate::llm::StreamEvent::Content(chunk) => {
                             if let Some(last) = app.messages.last_mut() {
                                if last.role == "assistant" {
                                    last.content.push_str(&chunk);
                                }
                            }
                            app.follow_bottom = true;
                        },
                        crate::llm::StreamEvent::Usage(usage) => {
                            app.last_request_tokens = usage.total_tokens;
                        }
                    }
                }
                LlmResult::StreamDone => {
                    app.is_thinking = false;
                    // We no longer strip think tags here so they can be toggled in UI
                    if let Some(last) = app.messages.last_mut() {
                         if last.role == "assistant" {
                             last.content = last.content.trim().to_string();
                         }
                    }
                }
                LlmResult::Error(e) => {
                    app.messages.push(ChatMessage { role: "assistant".to_string(), content: format!("Error: {}", e), thinking_collapsed: false });
                    app.is_thinking = false;
                    app.scroll_to_bottom();
                }
                LlmResult::ModelList(models) => {
                    app.available_models = models;
                    app.models_loading = false;
                    if !app.available_models.is_empty() {
                        // Find current model in list
                        let idx = app.available_models.iter().position(|m| m == &app.model_name).unwrap_or(0);
                        app.model_state.select(Some(idx));
                    }
                }
            }
        }
        
        // Check Sync results
        while let Ok(result) = rx_sync.try_recv() {
            match result {
                SyncResult::Log(msg) => {
                    app.sync_logs.push(msg);
                    app.scroll_to_bottom();
                }
                SyncResult::Success => {
                    app.sync_logs.push("✓ Sync Complete!".to_string());
                    app.sync_running = false;
                    app.sync_complete = true;
                    app.is_connected = state.poliformat.check_connection().await.unwrap_or(false);
                }
                SyncResult::Error(e) => {
                    app.sync_logs.push(format!("✗ Error: {}", e));
                    app.sync_running = false;
                    app.sync_complete = true;
                }
            }
        }
        
        // Check Login
        if let Ok(result) = rx_login.try_recv() {
            app.is_thinking = false;
            match result {
                LoginResult::Success => {
                    app.is_connected = state.poliformat.check_connection().await.unwrap_or(false);
                    app.login_error = None;
                    app.login_username.clear();
                    app.login_pin.clear();
                    app.mode = AppMode::Menu;
                    app.set_status(" ✓ Login Successful! ");
                }
                LoginResult::Error(e) => { app.login_error = Some(e); }
            }
        }
        
        // Check Reembed
        while let Ok(result) = rx_reembed.try_recv() {
            match result {
                ReembedResult::Progress(msg) => {
                    app.reembed_progress = msg;
                }
                ReembedResult::Complete(count) => {
                    app.reembed_running = false;
                    app.reembed_progress.clear();
                    app.rag_stats = Some(state.rag.get_stats());
                    app.set_status(format!(" ✓ Recalculated {} embeddings ", count));
                }
                ReembedResult::Error(e) => {
                    app.reembed_running = false;
                    app.reembed_progress = format!("Error: {}", e);
                }
            }
        }

        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match app.mode.clone() {
                        AppMode::Menu => handle_menu_input(&mut app, key.code, &state, &tx_sync, &tx_llm).await,
                        AppMode::Chat => handle_chat_input(&mut app, key, &state, &tx_llm).await,
                        AppMode::RagInfo => handle_rag_info_input(&mut app, key.code, &state, &tx_reembed).await,
                        AppMode::Login => handle_login_input(&mut app, key.code, &state, &tx_login).await,
                        AppMode::Sync => handle_sync_input(&mut app, key.code),
                        AppMode::Settings => handle_settings_input(&mut app, key.code, &state, &tx_llm).await,
                    }
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            if app.is_thinking || app.sync_running || app.models_loading || app.reembed_running { app.advance_throbber(); }
            
            // Auto-clear status message after 3 seconds
            if let Some(time) = app.status_message_time {
                if time.elapsed() >= Duration::from_secs(3) {
                    app.status_message = None;
                    app.status_message_time = None;
                }
            }
            
            last_tick = Instant::now();
        }

        if app.should_quit { break; }
    }

    restore_terminal(&mut terminal)?;
    Ok(())
}

// ============================================================================
// INPUT HANDLERS
// ============================================================================

async fn handle_menu_input(app: &mut TuiApp, key: KeyCode, state: &Arc<AppState>, tx_sync: &mpsc::Sender<SyncResult>, tx_llm: &mpsc::Sender<LlmResult>) {
    match key {
        KeyCode::Up => app.previous_menu_item(),
        KeyCode::Down => app.next_menu_item(),
        KeyCode::Enter => {
            if let Some(i) = app.menu_state.selected() {
                match i {
                    0 => { app.mode = AppMode::Chat; app.scroll_to_bottom(); },
                    1 => { // Sync
                        if !app.is_connected {
                            app.set_status(" ✗ Not connected! Login first. ");
                        } else {
                            app.mode = AppMode::Sync;
                            app.sync_logs.clear();
                            app.sync_running = true;
                            app.sync_complete = false;
                            app.sync_logs.push("Starting sync...".to_string());
                            
                            let tx = tx_sync.clone();
                            let rag = state.rag.clone();
                            let poliformat = state.poliformat.clone();
                            tokio::spawn(async move {
                                let _ = tx.send(SyncResult::Log("Fetching subjects...".to_string())).await;
                                match run_sync_with_logging(rag, poliformat, tx.clone()).await {
                                    Ok(_) => { let _ = tx.send(SyncResult::Success).await; },
                                    Err(e) => { let _ = tx.send(SyncResult::Error(e.to_string())).await; }
                                }
                            });
                        }
                    },
                    2 => { app.rag_stats = Some(state.rag.get_stats()); app.mode = AppMode::RagInfo; },
                    3 => { app.mode = AppMode::Login; app.login_field = 0; app.login_error = None; },
                    4 => { // Settings
                        app.mode = AppMode::Settings;
                        app.models_loading = true;
                        let tx = tx_llm.clone();
                        let llm = state.llm.lock().unwrap().clone();
                        tokio::spawn(async move {
                            match llm.fetch_models().await {
                                Ok(models) => { let _ = tx.send(LlmResult::ModelList(models)).await; },
                                Err(e) => { let _ = tx.send(LlmResult::Error(e.to_string())).await; }
                            }
                        });
                    },
                    5 => { app.should_quit = true; },
                    _ => {}
                }
            }
        },
        KeyCode::Esc => app.should_quit = true,
        _ => {}
    }
}

async fn handle_chat_input(app: &mut TuiApp, key: event::KeyEvent, state: &Arc<AppState>, tx_llm: &mpsc::Sender<LlmResult>) {
    match key.code {
        KeyCode::Esc => { app.mode = AppMode::Menu; },
        KeyCode::Enter => {
            if !app.input.trim().is_empty() && !app.is_thinking {
                let user_input = app.input.trim().to_string();
                app.input.clear();
                app.input_cursor = 0;
                
                if user_input.starts_with("/model") {
                    let parts: Vec<&str> = user_input.splitn(2, ' ').collect();
                    if parts.len() > 1 && !parts[1].trim().is_empty() {
                        let new_model = parts[1].trim().to_string();
                        state.llm.lock().unwrap().set_model(&new_model);
                        app.model_name = new_model.clone();
                        let _ = crate::config::Config::save_model(&new_model);
                        app.set_status(format!(" Model set: {} ", new_model));
                    } else {
                        // Show current model if no name provided
                        app.set_status(format!(" Current model: {} ", app.model_name));
                    }
                    return;
                }

                app.messages.push(ChatMessage { role: "user".to_string(), content: user_input.clone(), thinking_collapsed: false });
                // Placeholder for assistant
                app.messages.push(ChatMessage { role: "assistant".to_string(), content: String::new(), thinking_collapsed: false });
                app.scroll_to_bottom();
                app.is_thinking = true;
                app.status_message = None;
                
                let tx = tx_llm.clone();
                let rag = state.rag.clone();
                let llm = state.llm.lock().unwrap().clone();
                let messages = app.messages.clone();
                
                tokio::spawn(async move {
                    // Fetch more results for better coverage
                    let snippets = rag.search_snippets(&user_input, "user", 10).await.unwrap_or_default();
                    
                    tracing::info!("RAG search returned {} snippets for query: '{}'", snippets.len(), &user_input);
                    for (i, (source, snippet, score)) in snippets.iter().enumerate() {
                        tracing::debug!("Snippet {}: source='{}', score={:.3}, len={}", i, source, score, snippet.len());
                    }
                    
                    let mut context_str = String::new();
                    if !snippets.is_empty() {
                        context_str.push_str("Relevant context from your documents:\n");
                        for (source, snippet, _score) in snippets {
                            context_str.push_str(&format!("\n[{}]:\n{}\n", source, snippet));
                        }
                    }
                    let full = if !context_str.is_empty() { 
                        format!("{}\n\n---\nUser question: {}", context_str, user_input) 
                    } else { 
                        user_input 
                    };
                    
                    tracing::info!("Final prompt length: {} chars, has context: {}", full.len(), !context_str.is_empty());
                    
                    let mut mk = messages;
                    // Remove the empty assistant placeholder we added in UI thread
                    mk.pop();
                    
                    if let Some(l) = mk.last_mut() { 
                        tracing::debug!("Setting last message content (role: {})", l.role);
                        l.content = full.clone();
                    }
                    
                    tracing::debug!("Sending {} messages to LLM", mk.len());
                    for (i, m) in mk.iter().enumerate() {
                        tracing::debug!("  Msg {}: role='{}', content_len={}", i, m.role, m.content.len());
                    }
                    
                    match llm.chat_stream(&mk).await {
                         Ok(mut stream) => {
                            while let Some(chunk_res) = stream.next().await {
                                match chunk_res {
                                    Ok(event) => {
                                        let _ = tx.send(LlmResult::StreamChunk(event)).await;
                                    },
                                    Err(e) => {
                                         let _ = tx.send(LlmResult::Error(e.to_string())).await;
                                    }
                                }
                            }
                            let _ = tx.send(LlmResult::StreamDone).await;
                        },
                        Err(e) => {
                            let _ = tx.send(LlmResult::Error(e.to_string())).await;
                        }
                    }
                });
            }
        },
        KeyCode::Char(c) => { 
            if key.modifiers.contains(event::KeyModifiers::CONTROL) && c == 't' {
                 // Toggle thinking collapse for the last message if it has thinking
                 if let Some(last) = app.messages.last_mut() {
                     if last.role == "assistant" {
                         last.thinking_collapsed = !last.thinking_collapsed;
                         let msg = format!(" Thinking Process: {} ", if last.thinking_collapsed { "HIDDEN" } else { "SHOWN" });
                         app.status_message = Some(msg);
                         app.status_message_time = Some(Instant::now());
                     }
                 }
            } else if key.modifiers.contains(event::KeyModifiers::CONTROL) && c == 'l' {
                // Clear chat history (keep only system message)
                app.messages.retain(|m| m.role == "system");
                app.scroll_offset = 0;
                app.follow_bottom = true;
                app.set_status(" Chat history cleared ");
            } else if !app.is_thinking { 
                app.input.insert(app.input_cursor, c); 
                app.input_cursor += c.len_utf8(); 
            } 
        },
        KeyCode::Backspace => { 
            if !app.is_thinking && app.input_cursor > 0 { 
                // Find char boundary before cursor
                if let Some(prev_char_idx) = app.input[..app.input_cursor].char_indices().next_back().map(|(i, _)| i) {
                     app.input.remove(prev_char_idx);
                     app.input_cursor = prev_char_idx;
                }
            } 
        },
        KeyCode::Left => { 
            if app.input_cursor > 0 {
                if let Some((prev_idx, _)) = app.input[..app.input_cursor].char_indices().next_back() {
                    app.input_cursor = prev_idx;
                }
            }
        },
        KeyCode::Right => { 
            if app.input_cursor < app.input.len() { 
                 if let Some((next_idx, _)) = app.input[app.input_cursor..].char_indices().nth(1) {
                     app.input_cursor += next_idx;
                 } else {
                     app.input_cursor = app.input.len();
                 }
            } 
        },
        KeyCode::Up => { app.scroll_up(3); },
        KeyCode::Down => { app.scroll_down(3); },
        KeyCode::PageUp => { app.scroll_up(10); },
        KeyCode::PageDown => { app.scroll_down(10); },
        KeyCode::Home => { app.scroll_offset = 0; app.follow_bottom = false; },
        KeyCode::End => { app.scroll_to_bottom(); },
        _ => {}
    }
}

async fn handle_rag_info_input(app: &mut TuiApp, key: KeyCode, state: &Arc<AppState>, tx_reembed: &mpsc::Sender<ReembedResult>) {
    if app.reembed_running { return; }
    
    match key {
        KeyCode::Esc => { app.mode = AppMode::Menu; },
        KeyCode::Char('r') | KeyCode::Char('R') => {
            app.reembed_running = true;
            app.reembed_progress = "Starting...".to_string();
            
            let tx = tx_reembed.clone();
            let rag = state.rag.clone();
            
            tokio::spawn(async move {
                let result = rag.reembed_all(|current, total, id, metadata| {
                    let display_name = if let Some(filename) = metadata.get("filename") {
                        filename.clone()
                    } else if let Some(name) = metadata.get("name") {
                        name.clone()
                    } else {
                        // Fallback: Try to make ID/URL readable
                        if id.starts_with("http") || id.starts_with("/") {
                            if let Ok(url) = url::Url::parse(id) {
                                // Try to get the last path segment or something meaningful
                                if let Some(segments) = url.path_segments() {
                                    if let Some(last) = segments.last() {
                                        if !last.is_empty() {
                                             last.to_string()
                                        } else {
                                             id.to_string()
                                        }
                                    } else {
                                        id.to_string()
                                    }
                                } else {
                                    id.to_string()
                                }
                            } else {
                                // Just show last 30 chars?
                                if id.len() > 30 {
                                    format!("...{}", &id[id.len()-30..])
                                } else {
                                    id.to_string()
                                }
                            }
                        } else {
                             if id.len() > 30 { 
                                format!("{}...", &id[..30]) 
                            } else { 
                                id.to_string() 
                            }
                        }
                    };
                    
                    // Truncate if still too long
                    let final_name = if display_name.len() > 40 {
                        format!("{}...", &display_name[..40])
                    } else {
                        display_name
                    };
                    
                    let msg = format!("[{}/{}] {}", current, total, final_name);
                    // Note: Can't await in closure, so we send synchronously via try_send
                    let _ = tx.try_send(ReembedResult::Progress(msg));
                }).await;
                
                match result {
                    Ok(count) => { let _ = tx.send(ReembedResult::Complete(count)).await; },
                    Err(e) => { let _ = tx.send(ReembedResult::Error(e.to_string())).await; }
                }
            });
        },
        _ => {}
    }
}

fn handle_sync_input(app: &mut TuiApp, key: KeyCode) {
    match key {
        KeyCode::Esc => {
            if !app.sync_running { app.mode = AppMode::Menu; }
        },
        KeyCode::Up => app.scroll_up(3),
        KeyCode::Down => app.scroll_down(3),
        KeyCode::PageUp => app.scroll_up(10),
        KeyCode::PageDown => app.scroll_down(10),
        _ => {}
    }
}

async fn handle_settings_input(app: &mut TuiApp, key: KeyCode, state: &Arc<AppState>, tx_llm: &mpsc::Sender<LlmResult>) {
    // Handle text input for OpenRouter fields
    if app.settings_input_mode {
        match key {
            KeyCode::Esc => { app.settings_input_mode = false; },
            KeyCode::Enter => { app.settings_input_mode = false; },
            KeyCode::Backspace => {
                let target = if app.settings_field == 1 { &mut app.openrouter_key } else { &mut app.openrouter_model };
                target.pop();
            },
            KeyCode::Char(c) => {
                let target = if app.settings_field == 1 { &mut app.openrouter_key } else { &mut app.openrouter_model };
                target.push(c);
            },
            _ => {}
        }
        return;
    }

    match key {
        KeyCode::Esc => {
            // Save and Exit
            let provider = app.active_provider.clone();
            
            // Configure LLM
            {
                let mut llm = state.llm.lock().unwrap();
                llm.set_auth(provider.base_url(), Some(app.openrouter_key.clone()));
                if provider == crate::config::LlmProvider::OpenRouter {
                    if !app.openrouter_model.is_empty() {
                       llm.set_model(&app.openrouter_model);
                       app.model_name = app.openrouter_model.clone();
                    }
                } else if let Some(i) = app.model_state.selected() {
                    // If LM Studio and selection made, ensure it's set
                    if let Some(model) = app.available_models.get(i) {
                         llm.set_model(model);
                         app.model_name = model.clone();
                    }
                }
                
                // Fetch context limit for new model
                if let Ok(len) = llm.fetch_context_length().await {
                    app.context_limit = len;
                }
            }
            
            // Save config
            let _ = crate::config::Config::save_provider_config(
                provider, 
                Some(app.openrouter_key.clone()), 
                Some(app.openrouter_model.clone())
            );
            
            app.set_status(" Settings saved ");
            app.mode = AppMode::Menu;
        },
        KeyCode::Tab => {
            // Toggle Provider
            app.active_provider = match app.active_provider {
                crate::config::LlmProvider::LmStudio => crate::config::LlmProvider::OpenRouter,
                crate::config::LlmProvider::OpenRouter => crate::config::LlmProvider::LmStudio,
            };
            app.settings_field = 0; // Reset focus
            
            // Refetch models for the new provider
            app.available_models.clear();
            app.models_loading = true;
            
            // Create a temporary client configuration
            let provider = app.active_provider.clone();
            let base_url = provider.base_url().to_string();
            let api_key = if provider == crate::config::LlmProvider::OpenRouter {
                Some(app.openrouter_key.clone()) // Use the key currently in the input field
            } else {
                None
            };
            
            let tx = tx_llm.clone();
            tokio::spawn(async move {
                // Use a temporary client to fetch models
                let client = crate::llm::LlmClient::new(Some(base_url), None, api_key);
                match client.fetch_models().await {
                    Ok(models) => { let _ = tx.send(LlmResult::ModelList(models)).await; },
                    Err(e) => { let _ = tx.send(LlmResult::Error(e.to_string())).await; }
                }
            });
        },
        KeyCode::Up => {
            if app.active_provider == crate::config::LlmProvider::LmStudio {
                app.previous_model();
            } else {
                if app.settings_field > 0 { app.settings_field -= 1; }
            }
        },
        KeyCode::Down => {
            if app.active_provider == crate::config::LlmProvider::LmStudio {
                 app.next_model();
            } else {
                if app.settings_field < 2 { app.settings_field += 1; }
            }
        },
        KeyCode::Enter => {
            if app.active_provider == crate::config::LlmProvider::LmStudio {
                if let Some(i) = app.model_state.selected() {
                    if let Some(model) = app.available_models.get(i) {
                        let new_model = model.clone();
                        
                        // update global state
                        {
                            let mut llm = state.llm.lock().unwrap();
                            llm.set_model(&new_model);
                            llm.set_auth(crate::config::LlmProvider::LmStudio.base_url(), None);
                        }
                        
                        app.model_name = new_model.clone();
                        
                        // Save config
                        let _ = crate::config::Config::save_model(&new_model);
                        let _ = crate::config::Config::save_provider_config(
                            crate::config::LlmProvider::LmStudio,
                            None,
                            None
                        );
                        
                        app.set_status(format!(" Model set to: {} ", new_model));
                        app.mode = AppMode::Menu;
                    }
                }
            } else {
                // Enter edit mode for fields > 0
                if app.settings_field > 0 {
                    app.settings_input_mode = true;
                }
            }
        },
        _ => {}
    }
}

async fn handle_login_input(app: &mut TuiApp, key: KeyCode, state: &Arc<AppState>, tx_login: &mpsc::Sender<LoginResult>) {
    if app.is_thinking { return; }
    match key {
        KeyCode::Esc => { app.mode = AppMode::Menu; app.login_username.clear(); app.login_pin.clear(); app.login_error = None; },
        KeyCode::Tab => { app.login_field = (app.login_field + 1) % 2; },
        KeyCode::Enter => {
            if !app.login_username.is_empty() && !app.login_pin.is_empty() {
                app.is_thinking = true;
                app.login_error = None;
                let tx = tx_login.clone();
                let client = state.poliformat.clone();
                let username = app.login_username.clone();
                let pin = app.login_pin.clone();
                tokio::task::spawn_blocking(move || {
                    let creds = crate::scrapper::auth::AuthCredentials { username: username.clone(), pin: pin.clone() };
                    let result = match client.login_headless(&creds) {
                        Ok(_) => { let _ = crate::config::Config::save_credentials(&username, &pin); LoginResult::Success },
                        Err(e) => LoginResult::Error(e.to_string()),
                    };
                    let rt = tokio::runtime::Handle::current();
                    rt.block_on(async { let _ = tx.send(result).await; });
                });
            } else { app.login_error = Some("Please fill in both fields".to_string()); }
        },
        KeyCode::Char(c) => { if app.login_field == 0 { app.login_username.push(c); } else { app.login_pin.push(c); } },
        KeyCode::Backspace => { if app.login_field == 0 { app.login_username.pop(); } else { app.login_pin.pop(); } },
        _ => {}
    }
}

async fn run_sync_with_logging(
    rag: Arc<crate::rag::RagSystem>,
    poliformat: Arc<crate::scrapper::PoliformatClient>,
    tx: mpsc::Sender<SyncResult>,
) -> anyhow::Result<()> {
    let _ = tx.send(SyncResult::Log("🗑️  Clearing old RAG index...".to_string())).await;
    rag.clear()?;
    
    let data_dir = crate::config::Config::get_scraped_data_dir();
    if data_dir.exists() {
        let _ = tx.send(SyncResult::Log("🗑️  Removing old data directory...".to_string())).await;
        let _ = std::fs::remove_dir_all(&data_dir);
    }
    
    let _ = tx.send(SyncResult::Log("🔍 Fetching subjects from PoliformaT...".to_string())).await;
    let subjects = poliformat.get_subjects().await?;
    let total = subjects.len();
    let _ = tx.send(SyncResult::Log(format!("📚 Found {} subjects", total))).await;
    
    let _ = tx.send(SyncResult::Log("📥 Starting content scrape...".to_string())).await;
    
    // Clone subjects for the progress tracking
    let subject_names: Vec<String> = subjects.iter().map(|s| s.name.clone()).collect();
    
    // Log each subject we're about to scrape
    for (i, name) in subject_names.iter().enumerate() {
        let _ = tx.send(SyncResult::Log(format!("[{}/{}] Queued: {}", i + 1, total, name))).await;
    }
    
    let _ = tx.send(SyncResult::Log("⏳ Scraping content (this may take a while)...".to_string())).await;
    let detailed_subjects = poliformat.scrape_subject_content(subjects).await?;
    let _ = tx.send(SyncResult::Log("✅ Downloads complete!".to_string())).await;
    
    let indexing_total = detailed_subjects.len();
    for (i, (sub, dir_path)) in detailed_subjects.iter().enumerate() {
        let _ = tx.send(SyncResult::Log(format!("[{}/{}] 📖 Indexing: {}", i + 1, indexing_total, sub.name))).await;
        
        let summary_path = std::path::Path::new(&dir_path).join("summary.md");
        let mut content = if summary_path.exists() {
            std::fs::read_to_string(&summary_path).unwrap_or_default()
        } else {
            let _ = tx.send(SyncResult::Log(format!("  ⚠️  No summary found, skipping..."))).await;
            continue;
        };
        
        let resources_path = std::path::Path::new(&dir_path).join("resources");
        let mut file_count = 0;
        if resources_path.exists() {
            use std::fmt::Write;
            let mut file_list = String::new();
            writeln!(&mut file_list, "\n\n[Local Files]:").unwrap();
            if let Ok(entries) = std::fs::read_dir(&resources_path) {
                for entry in entries.flatten() {
                    if let Ok(name) = entry.file_name().into_string() {
                        writeln!(&mut file_list, "- {}", name).unwrap();
                        file_count += 1;
                    }
                }
            }
            content.push_str(&file_list);
        }
        
        if file_count > 0 {
            let _ = tx.send(SyncResult::Log(format!("  📁 Found {} resource files", file_count))).await;
        }
        
        let _ = tx.send(SyncResult::Log(format!("  🔄 Processing PDFs..."))).await;
        let extracted_docs = crate::scrapper::processing::process_resources(std::path::Path::new(&dir_path)).unwrap_or_default();
        
        let full_text = format!("Subject: {}\nURL: {}\n\n{}", sub.name, sub.url, content);
        rag.add_document(&sub.id, &full_text, "user", [("type".to_string(), "subject".to_string())].into()).await?;
        
        if !extracted_docs.is_empty() {
            let _ = tx.send(SyncResult::Log(format!("  📄 Indexing {} PDFs...", extracted_docs.len()))).await;
        }
        
        for (rel_path, text) in extracted_docs {
            let doc_id = format!("{}/{}", sub.id, rel_path);
            let pdf_text = format!("Subject: {}\nFile: {}\n\n{}", sub.name, rel_path, text);
            rag.add_document(&doc_id, &pdf_text, "user", [("type".to_string(), "pdf".to_string()), ("filename".to_string(), rel_path)].into()).await?;
        }
        
        let _ = tx.send(SyncResult::Log(format!("  ✓ Done: {}", sub.name))).await;
    }
    
    let stats = rag.get_stats();
    let _ = tx.send(SyncResult::Log(format!("📊 Final index: {} documents, {}", stats.document_count, stats.format_file_size()))).await;
    
    Ok(())
}



fn draw_settings(frame: &mut Frame, app: &mut TuiApp) {
    let size = frame.area();
    
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" Settings ");
    let inner_area = block.inner(size);
    frame.render_widget(block, size);
    
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(6), // Logo
            Constraint::Length(3), // Provider Select
            Constraint::Length(3), // Input 1 (Model List or API Key)
            Constraint::Length(3), // Input 2 (Model Name)
            Constraint::Min(3),    // Remaining/Help
        ])
        .margin(1)
        .split(inner_area);
    
    frame.render_widget(Paragraph::new(render_logo()).alignment(Alignment::Center), layout[0]);
    
    // 1. Provider Selection
    let provider_style = if app.settings_field == 0 { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::White) };
    let lm_style = if app.active_provider == crate::config::LlmProvider::LmStudio { Style::default().bg(Color::Blue).fg(Color::White) } else { Style::default() };
    let or_style = if app.active_provider == crate::config::LlmProvider::OpenRouter { Style::default().bg(Color::Blue).fg(Color::White) } else { Style::default() };
    
    let provider_span = Line::from(vec![
        Span::styled(" Provider: ", provider_style),
        Span::styled(" [ LM Studio ] ", lm_style),
        Span::raw("   "),
        Span::styled(" [ OpenRouter ] ", or_style),
    ]);
    frame.render_widget(Paragraph::new(provider_span).alignment(Alignment::Center), layout[1]);
    
    match app.active_provider {
        crate::config::LlmProvider::LmStudio => {
             let model_style = if app.settings_field == 1 { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::Green) };
             frame.render_widget(
                 Paragraph::new(format!("Current Model: {}", app.model_name)).style(model_style).alignment(Alignment::Center),
                 layout[2]
             );
             
            if app.models_loading {
                frame.render_widget(Paragraph::new("Loading models...").alignment(Alignment::Center), layout[3]);
            } else if app.available_models.is_empty() {
                frame.render_widget(Paragraph::new("No models found. Is your LLM server running?").style(Style::default().fg(Color::Red)).alignment(Alignment::Center), layout[3]);
            } else {
                let items: Vec<ListItem> = app.available_models.iter()
                    .map(|m| {
                        let style = if m == &app.model_name { Style::default().fg(Color::Green).add_modifier(Modifier::BOLD) } else { Style::default() };
                        ListItem::new(Line::from(vec![Span::styled(m, style)]))
                    })
                    .collect();
                
                let list = List::new(items)
                    .block(Block::default().borders(Borders::ALL).title(" Available Models "))
                    .highlight_style(Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD));
                
                // Allow list to take up remaining space
                let list_area = Layout::default().direction(Direction::Vertical).constraints([Constraint::Min(5)]).split(layout[3].union(layout[4]))[0];
                 // Use horizontal padding for the list
                let model_layout = Layout::default().direction(Direction::Horizontal).constraints([Constraint::Percentage(15), Constraint::Percentage(70), Constraint::Percentage(15)]).split(list_area);
                frame.render_stateful_widget(list, model_layout[1], &mut app.model_state);
            }
        },
        crate::config::LlmProvider::OpenRouter => {
            // API Key Input
            let key_style = if app.settings_field == 1 { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::White) };
            let key_border = if app.settings_field == 1 && app.settings_input_mode { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };
            
            let key_display = if app.openrouter_key.is_empty() { "Enter API Key..." } else { "****************" };
            let key_widget = Paragraph::new(if app.settings_field == 1 && app.settings_input_mode { app.openrouter_key.as_str() } else { key_display })
                .block(Block::default().borders(Borders::ALL).border_style(key_border).title(" OpenRouter API Key "))
                .style(key_style);
            frame.render_widget(key_widget, layout[2]);
            
            // Model Name Input
            let model_style = if app.settings_field == 2 { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::White) };
            let model_border = if app.settings_field == 2 && app.settings_input_mode { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };
            
            let model_widget = Paragraph::new(app.openrouter_model.as_str())
                .block(Block::default().borders(Borders::ALL).border_style(model_border).title(" Model Name (e.g. google/gemini-2.0-flash-001) "))
                .style(model_style);
            frame.render_widget(model_widget, layout[3]);
            
            // Instructions
            let instr = Paragraph::new("Tab: Switch Provider | Up/Down: Select Field | Enter: Edit | Esc: Cancel/Save")
                .style(Style::default().fg(Color::DarkGray))
                .alignment(Alignment::Center);
             frame.render_widget(instr, layout[4]);
        }
    }
}
