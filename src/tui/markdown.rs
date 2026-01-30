use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use tui_markdown::from_str;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};


pub fn render_markdown(text: &str, max_width: usize, thinking_collapsed: bool) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // 1. Separate Thinking Block
    // We assume <think> is at the start if present (standard deep-think pattern)
    // and extract it to render manually (so we can toggle it).
    let (thinking_content, main_content_raw) = if let Some(start) = text.find("<think>") {
        if let Some(end) = text[start..].find("</think>") {
            let think_end = start + end + 8; // length of </think> is 8
            let think_inner = &text[start + 7..start + end];
            (Some(think_inner), &text[think_end..])
        } else {
             // Open thinking tag but no close (streaming)
             let think_inner = &text[start + 7..];
             (Some(think_inner), "")
        }
    } else {
        (None, text)
    };

    // 2. Render Thinking Block
    if let Some(think) = thinking_content {
        lines.push(Line::from(""));
        
        // Header
        let header_style = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
        let icon = if thinking_collapsed { "▶" } else { "▼" };
        lines.push(Line::from(vec![
             Span::styled(format!(" {} Thinking Process", icon), header_style)
        ]));
        
        // Content (only if expanded)
        if !thinking_collapsed {
            let think_style = Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC);
            // We can just simple-wrap the thinking text since it's usually raw thoughts.
            // Or we could run it through the markdown renderer too if we wanted, 
            // but usually raw is fine and safer for stream.
            let wrapped = wrap_text_simple(think, max_width);
            for w in wrapped {
                lines.push(Line::from(Span::styled(w, think_style)));
            }
            lines.push(Line::from(""));
        }
    }

    // 3. Pre-process Main Content for ASCII Tables
    // tui-markdown will wrap text that looks like paragraphs.
    // ASCII tables look like paragraphs to it (just lines of text).
    // We need to wrap them in code blocks ```text ... ``` so they are preserved verbatim.
    // ALSO: Detect standard GFM tables and convert them to ASCII Art code blocks since tui-markdown doesn't support them.
    let gfm_processed = preprocess_gfm_tables(main_content_raw, max_width);
    let processed_content = preprocess_ascii_tables(&gfm_processed);

    // 4. Render Main Content using tui-markdown
    // tui-markdown returns a Text<'a>, we convert to Vec<Line>
    // tui-markdown returns a Text<'a>, we convert to Vec<Line>
    let rendered_text = from_str(&processed_content);
    
    // Convert rendered_text to our Vec<Line> (and apply max_width via wrapping if needed?)
    // tui-markdown doesn't do wrapping by default, correct? 
    // Wait, the documentation says "The Text widget can then be rendered...".
    // Text widget in Ratatui wraps automatically if we render it in a paragraph with wrap.
    // But here we are returning Vec<Line> to be put in a List or similar.
    // If we put it in a List, it won't wrap automatically.
    // Actually, tui-markdown output is likely unwrapped lines.
    // For now, let's just push them. If wrapping is needed, Ratatui's Paragraph/List might need help.
    // IMPORTANT: The user's TUI likely uses a List of lines. We might need to manual wrap paragraphs?
    // Let's assume tui-markdown handles basic wrapping *or* we rely on the TUI widget to wrap.
    // Checking the user's previous code, `render_markdown` returned `Vec<Line>`.
    // And `wrap_text` was manually called.
    // tui-markdown does NOT wrap. 
    // However, implementing manual wrapping on `tui-markdown` output is hard because we lose semantic info.
    // BUT! `tui-markdown` just produces styled lines.
    // Let's trust that for now, but if wrapping is broken for normal text, we might need to 
    // wrap the *input* markdown or post-process.
    // Actually, `tui-markdown` might not wrap.
    // Let's rely on standard behavior first.
    
    // Convert rendered_text to owned lines to escape the lifetime of processed_content
    // 5. Post-process lines to remove "polirag_table" fences
    // tui-markdown renders fences for code blocks. We want to hide them for our auto-generated tables.
    let lines_vec: Vec<Line> = rendered_text.lines.into_iter().map(|l| convert_core_line(l)).collect();
    
    // Filter out the fences
    // We look for lines that consist exactly of "```polirag_table" or "```" (closing).
    let mut cleaned_lines = Vec::new();
    let mut in_polirag_table = false;
    
    for line in lines_vec {
        let text_content = line.to_string(); // Helper or spans join
        if text_content.trim() == "```polirag_table" {
            in_polirag_table = true;
            continue; // Skip fence
        }
        if in_polirag_table && text_content.trim() == "```" {
            in_polirag_table = false;
            continue; // Skip fence
        }
        cleaned_lines.push(line);
    }
    lines.extend(cleaned_lines);

    lines
}

fn convert_core_line(line: ratatui_core::text::Line<'_>) -> Line<'static> {
    let spans: Vec<Span<'static>> = line.spans.into_iter().map(|s| {
        // Convert core Span to standard Span
        // We need to convert Style too if they are different types (they likely are distinct)
        // Fortunately Style usually implements Into or has same fields.
        let core_style = s.style;
        let style = convert_core_style(core_style);
        
        Span::styled(s.content.into_owned(), style)
    }).collect();
    // Alignment might be lost if we don't map it, but Line::from(spans) defaults to Left.
    // ratatui_core::Line has alignment field.
    let new_line = Line::from(spans);
    // basic mapping for alignment if needed, but usually markdown is left aligned.
    new_line
}

fn convert_core_style(style: ratatui_core::style::Style) -> Style {
    let mut s = Style::default();
    
    if let Some(fg) = style.fg { s = s.fg(convert_core_color(fg)); }
    if let Some(bg) = style.bg { s = s.bg(convert_core_color(bg)); }
    
    s = s.add_modifier(convert_core_modifier(style.add_modifier));
    s = s.remove_modifier(convert_core_modifier(style.sub_modifier));
    s
}

fn convert_core_color(c: ratatui_core::style::Color) -> Color {
    // Enum mapping
    match c {
        ratatui_core::style::Color::Reset => Color::Reset,
        ratatui_core::style::Color::Black => Color::Black,
        ratatui_core::style::Color::Red => Color::Red,
        ratatui_core::style::Color::Green => Color::Green,
        ratatui_core::style::Color::Yellow => Color::Yellow,
        ratatui_core::style::Color::Blue => Color::Blue,
        ratatui_core::style::Color::Magenta => Color::Magenta,
        ratatui_core::style::Color::Cyan => Color::Cyan,
        ratatui_core::style::Color::Gray => Color::Gray,
        ratatui_core::style::Color::DarkGray => Color::DarkGray,
        ratatui_core::style::Color::LightRed => Color::LightRed,
        ratatui_core::style::Color::LightGreen => Color::LightGreen,
        ratatui_core::style::Color::LightYellow => Color::LightYellow,
        ratatui_core::style::Color::LightBlue => Color::LightBlue,
        ratatui_core::style::Color::LightMagenta => Color::LightMagenta,
        ratatui_core::style::Color::LightCyan => Color::LightCyan,
        ratatui_core::style::Color::White => Color::White,
        ratatui_core::style::Color::Indexed(i) => Color::Indexed(i),
        ratatui_core::style::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

fn convert_core_modifier(m: ratatui_core::style::Modifier) -> Modifier {
    let mut modifier = Modifier::empty();
    if m.contains(ratatui_core::style::Modifier::BOLD) { modifier |= Modifier::BOLD; }
    if m.contains(ratatui_core::style::Modifier::DIM) { modifier |= Modifier::DIM; }
    if m.contains(ratatui_core::style::Modifier::ITALIC) { modifier |= Modifier::ITALIC; }
    if m.contains(ratatui_core::style::Modifier::UNDERLINED) { modifier |= Modifier::UNDERLINED; }
    if m.contains(ratatui_core::style::Modifier::SLOW_BLINK) { modifier |= Modifier::SLOW_BLINK; }
    if m.contains(ratatui_core::style::Modifier::RAPID_BLINK) { modifier |= Modifier::RAPID_BLINK; }
    if m.contains(ratatui_core::style::Modifier::REVERSED) { modifier |= Modifier::REVERSED; }
    if m.contains(ratatui_core::style::Modifier::HIDDEN) { modifier |= Modifier::HIDDEN; }
    if m.contains(ratatui_core::style::Modifier::CROSSED_OUT) { modifier |= Modifier::CROSSED_OUT; }
    modifier
}

/// Wraps ASCII tables/box-drawings in ```text code blocks to prevent markdown parsing from butchering them
fn preprocess_ascii_tables(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut in_ascii_block = false;
    let mut in_md_code_block = false;

    for line in text.lines() {
        let trimmed = line.trim();
        
        // Track if we are inside a standard markdown code block already
        if trimmed.starts_with("```") {
            in_md_code_block = !in_md_code_block;
            // If we were effectively buffering an ascii block and hit a code fence,
            // close ascii block first? standard markdown shouldn't mix, but just in case.
            if in_ascii_block {
                result.push_str("```\n");
                in_ascii_block = false;
            }
            result.push_str(line);
            result.push('\n');
            continue;
        }
        
        if in_md_code_block {
            result.push_str(line);
            result.push('\n');
            continue;
        }

        // Detect ASCII Art Table line
        // Must contain box drawing chars.
        // Also check for leading pipe if it has box chars inside (hybrid table fix)
        let is_ascii_line = line.contains('┌') || line.contains('│') || line.contains('└') 
                          || line.contains('├') || line.contains('─') || line.contains('┬') || line.contains('┴') || line.contains('┼');

        if is_ascii_line {
             if !in_ascii_block {
                 result.push_str("\n```polirag_table\n"); // Start code block with hidden tag
                 in_ascii_block = true;
             }
             result.push_str(line);
             result.push('\n');
        } else {
             if in_ascii_block {
                 result.push_str("```\n"); // End code block
                 in_ascii_block = false;
             }
             result.push_str(line);
             result.push('\n');
        }
    }
    
    // Close pending block
    if in_ascii_block {
        result.push_str("```\n");
    }

    result
}

// Simple wrapper for the Thinking block (gray text)
fn wrap_text_simple(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for line in text.lines() {
        if line.chars().count() > width {
             // Basic hard wrap
             let mut current = String::new();
             let mut count = 0;
             for c in line.chars() {
                 if count >= width {
                     lines.push(current);
                     current = String::new();
                     count = 0;
                 }
                 current.push(c);
                 count += 1;
             }
             if !current.is_empty() {
                 lines.push(current);
             }
        } else {
            lines.push(line.to_string());
        }
    }
    lines
}
fn preprocess_gfm_tables(text: &str, max_width: usize) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    
    let mut replacements = Vec::new();
    let mut current_table_start = None;
    let mut in_table = false;
    let mut table_events = Vec::new();
    
    let parser = Parser::new_ext(text, options);
    let iter = parser.into_offset_iter();
    
    for (event, range) in iter {
        match event {
            Event::Start(Tag::Table(_)) => {
                in_table = true;
                current_table_start = Some(range.start);
                table_events.clear();
                table_events.push(event);
            }
            Event::End(TagEnd::Table) => {
                if in_table {
                    let start = current_table_start.unwrap_or(range.start);
                    let end = range.end;
                    
                    table_events.push(event);
                    
                    // Render the buffered events into an ASCII table string
                    let ascii_table = render_table_from_events(&table_events, max_width);
                    replacements.push((start, end, ascii_table));
                    
                    in_table = false;
                    current_table_start = None;
                    table_events.clear();
                }
            }
            _ => {
                if in_table {
                    table_events.push(event);
                }
            }
        }
    }
    
    // Apply replacements in reverse order to keep offsets valid
    let mut result = text.to_string();
    for (start, end, replacement) in replacements.into_iter().rev() {
        if start < result.len() && end <= result.len() {
             result.replace_range(start..end, &replacement);
        }
    }
    
    result
}

fn render_table_from_events(events: &[Event], max_width: usize) -> String {
    // Reconstruct table data
    let mut rows = Vec::new();
    let mut current_row = Vec::new();
    let mut in_cell = false;
    let mut cell_content = String::new();
    
    for event in events {
        match event {
            Event::Start(Tag::TableRow) => {
                current_row = Vec::new();
            }
            Event::End(TagEnd::TableRow) => {
                rows.push(current_row.clone());
            }
            Event::Start(Tag::TableCell) | Event::Start(Tag::TableHead) => { 
                in_cell = true;
                cell_content.clear();
            }
            Event::End(TagEnd::TableCell) => {
                current_row.push(cell_content.trim().to_string());
                in_cell = false;
            }
            Event::Text(t) => {
                if in_cell {
                    cell_content.push_str(&t);
                }
            }
            Event::Code(c) => {
                 if in_cell {
                     cell_content.push('`');
                     cell_content.push_str(&c);
                     cell_content.push('`');
                 }
            }
            _ => {
                if in_cell {
                     if let Event::SoftBreak = event {
                         cell_content.push(' ');
                     } else if let Event::HardBreak = event {
                         cell_content.push('\n');
                     }
                }
            }
        }
    }

    if rows.is_empty() { return String::new(); }
    
    let num_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if num_cols == 0 { return String::new(); }

    // Calculate maximum content width for each column
    let mut max_content_widths = vec![0; num_cols];
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            if i < num_cols {
                max_content_widths[i] = max_content_widths[i].max(cell.chars().count());
            }
        }
    }

    // Determine final column widths based on available max_width
    // Basic structure: | cell | cell | -> chars = sum(widths) + (num_cols * 3) + 1
    // Padding: " " + text + " " = width + 2. Border: | (num_cols + 1).
    let overhead = (num_cols * 3) + 1; 
    let available_content_width = max_width.saturating_sub(overhead);

    let total_max_content: usize = max_content_widths.iter().sum();
    
    let final_col_widths: Vec<usize> = if total_max_content <= available_content_width {
        max_content_widths
    } else {
        // Distribute available width proportionally
        // Ensure at least min_width chars per column
        let min_col_width = 10;
        let mut widths = vec![min_col_width; num_cols];
        let remaining = available_content_width.saturating_sub(num_cols * min_col_width);
        
        if remaining > 0 {
             // Distribute remaining proportionally to need
             let mutable_content_sum: usize = max_content_widths.iter().map(|&w| w.saturating_sub(min_col_width)).sum();
             if mutable_content_sum > 0 {
                 for i in 0..num_cols {
                     let extra_need = max_content_widths[i].saturating_sub(min_col_width);
                     let share = (remaining as f64 * (extra_need as f64 / mutable_content_sum as f64)) as usize;
                     widths[i] += share;
                 }
             } else {
                 // Distribute evenly if everyone is small (unlikely path)
                  let share = remaining / num_cols;
                  for w in widths.iter_mut() { *w += share; }
             }
        }
        widths
    };

    // Render
    let mut out = String::new();
    out.push_str("\n```polirag_table\n");
    
    // Top Border
    out.push('┌');
    for (i, w) in final_col_widths.iter().enumerate() {
        out.push_str(&"─".repeat(w + 2));
        if i < num_cols - 1 { out.push('┬'); }
    }
    out.push('┐');
    out.push('\n');
    
    for (row_idx, row) in rows.iter().enumerate() {
        // Wrap cell contents
        let mut wrapped_cells: Vec<Vec<String>> = Vec::new();
        let mut max_row_height = 0;
        
        for i in 0..num_cols {
            let cell_text = row.get(i).map(|s| s.as_str()).unwrap_or("");
            let width = final_col_widths[i];
            let wrapped = textwrap::wrap(cell_text, width);
            let lines: Vec<String> = if wrapped.is_empty() {
                if cell_text.is_empty() { vec![String::new()] } else { vec![cell_text.to_string()] }
            } else {
                wrapped.into_iter().map(|c| c.to_string()).collect()
            };
            max_row_height = max_row_height.max(lines.len());
            wrapped_cells.push(lines);
        }
        
        // Print row lines
        for line_idx in 0..max_row_height {
            out.push('│');
            for (col_idx, w) in final_col_widths.iter().enumerate() {
                let cell_lines = &wrapped_cells[col_idx];
                let text = if line_idx < cell_lines.len() { &cell_lines[line_idx] } else { "" };
                
                out.push(' ');
                out.push_str(text);
                out.push_str(&" ".repeat(w.saturating_sub(text.chars().count())));
                out.push(' ');
                out.push('│');
            }
            out.push('\n');
        }
        
        // Separator between all rows for clear definition
        if row_idx < rows.len() - 1 {
            out.push('├');
            for (i, w) in final_col_widths.iter().enumerate() {
                out.push_str(&"─".repeat(w + 2));
                if i < num_cols - 1 { out.push('┼'); }
            }
            out.push('┤');
            out.push('\n');
        }
    }
    
    // Bottom Border
    out.push('└');
    for (i, w) in final_col_widths.iter().enumerate() {
        out.push_str(&"─".repeat(w + 2));
        if i < num_cols - 1 { out.push('┴'); }
    }
    out.push('┘');
    
    out.push_str("\n```\n");
    out
}
