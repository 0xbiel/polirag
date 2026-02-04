use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
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
                lines.push(Line::from(Span::styled(format!("   {}", w), think_style)));
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

    // 4. Custom Markdown Rendering
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    let parser = Parser::new_ext(&processed_content, options);

    let mut current_line = Vec::new();
    let mut style_stack = vec![Style::default()];
    let mut list_index = vec![];
    let mut list_depth = 0;
    let mut in_code_block = false;

    for event in parser {
        match event {
            Event::Start(tag) => {
                match tag {
                    Tag::Paragraph => {
                        if !lines.is_empty() || !current_line.is_empty() { lines.push(Line::from("")); }
                    }
                    Tag::Heading { level, .. } => {
                        if !lines.is_empty() || !current_line.is_empty() { lines.push(Line::from("")); }
                        let color = match level {
                            pulldown_cmark::HeadingLevel::H1 => Color::Cyan,
                            pulldown_cmark::HeadingLevel::H2 => Color::Blue,
                            pulldown_cmark::HeadingLevel::H3 => Color::Magenta,
                            _ => Color::White,
                        };
                        style_stack.push(Style::default().fg(color).add_modifier(Modifier::BOLD));
                    }
                    Tag::List(start) => {
                        list_depth += 1;
                        list_index.push(start);
                    }
                    Tag::Item => {
                        let indent = "  ".repeat(list_depth - 1);
                        let bullet = match list_depth {
                            1 => "• ",
                            2 => "◦ ",
                            _ => "▪ ",
                        };
                        current_line.push(Span::raw(format!("{}{}", indent, bullet)));
                    }
                    Tag::Emphasis => {
                        let current = *style_stack.last().unwrap();
                        style_stack.push(current.add_modifier(Modifier::ITALIC));
                    }
                    Tag::Strong => {
                        let current = *style_stack.last().unwrap();
                        style_stack.push(current.add_modifier(Modifier::BOLD));
                    }
                    Tag::Strikethrough => {
                        let current = *style_stack.last().unwrap();
                        style_stack.push(current.add_modifier(Modifier::CROSSED_OUT));
                    }
                    Tag::CodeBlock(_) => {
                        in_code_block = true;
                        lines.push(Line::from(""));
                    }
                    Tag::BlockQuote(_) => {
                        style_stack.push(Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC));
                        current_line.push(Span::styled("▎ ", Style::default().fg(Color::Blue)));
                    }
                    _ => {}
                }
            }
            Event::End(tag) => {
                match tag {
                    TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item => {
                        let is_heading = matches!(tag, TagEnd::Heading(_));
                        if is_heading { style_stack.pop(); }
                        let line = Line::from(current_line.drain(..).collect::<Vec<_>>());
                        // Simple wrapping for lines
                        for wrapped in wrap_line(line, max_width) {
                            lines.push(wrapped);
                        }
                    }
                    TagEnd::List(_) => {
                        list_depth -= 1;
                        list_index.pop();
                    }
                    TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::BlockQuote(_) => {
                        style_stack.pop();
                    }
                    TagEnd::CodeBlock => {
                        in_code_block = false;
                        lines.push(Line::from(""));
                    }
                    _ => {}
                }
            }
            Event::Text(t) => {
                if in_code_block {
                    for line in t.lines() {
                        lines.push(Line::from(Span::styled(format!("  {}", line), Style::default().fg(Color::DarkGray))));
                    }
                } else {
                    current_line.push(Span::styled(t.into_string(), *style_stack.last().unwrap()));
                }
            }
            Event::Code(c) => {
                current_line.push(Span::styled(c.into_string(), Style::default().fg(Color::Yellow)));
            }
            Event::SoftBreak | Event::HardBreak => {
                current_line.push(Span::raw(" "));
            }
            _ => {}
        }
    }

    // Clean up empty lines at start/end
    while lines.first().map_or(false, |l| l.to_string().trim().is_empty()) { lines.remove(0); }
    while lines.last().map_or(false, |l| l.to_string().trim().is_empty()) { lines.pop(); }

    lines
}

fn wrap_line(line: Line<'static>, width: usize) -> Vec<Line<'static>> {
    let mut result = Vec::new();
    let mut current_spans = Vec::new();
    let mut current_width = 0;

    for span in line.spans {
        let content = span.content.as_ref();
        let style = span.style;
        let words: Vec<&str> = content.split_inclusive(' ').collect();

        for word in words {
            let word_len = word.chars().count();
            if current_width + word_len > width && !current_spans.is_empty() {
                result.push(Line::from(current_spans));
                current_spans = Vec::new();
                current_width = 0;
            }
            current_spans.push(Span::styled(word.to_string(), style));
            current_width += word_len;
        }
    }
    if !current_spans.is_empty() {
        result.push(Line::from(current_spans));
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
                 result.push_str("\n```text\n"); // Start code block with hidden tag
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
    
    out.push('\n');
    out
}
