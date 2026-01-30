use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

pub fn render_markdown(text: &str, max_width: usize, thinking_collapsed: bool) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut in_code_block = false;
    let mut table_lines = Vec::new();
    
    // Split text into thinking and content parts
    // We assume <think> is at the start if present
    let (thinking_content, main_content) = if let Some(start) = text.find("<think>") {
        if let Some(end) = text[start..].find("</think>") {
            let think_end = start + end + 8;
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

    // Render Thinking Block
    if let Some(think) = thinking_content {
        lines.push(Line::from(""));
        
        let header_style = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
        let icon = if thinking_collapsed { "▶" } else { "▼" };
        lines.push(Line::from(vec![
             Span::styled(format!(" {} Thinking Process", icon), header_style)
        ]));
        
        if !thinking_collapsed {
            let think_style = Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC);
            // Render thinking content with gray style
             // We can use a simpler wrapping since we don't need full markdown inside think block usually
             // But let's support basic wrapping
             for line in think.lines() {
                 let wrapped = wrap_text(line, max_width, 2);
                 for w in wrapped {
                     lines.push(Line::from(Span::styled(w, think_style)));
                 }
             }
             lines.push(Line::from(""));
        }
    }

    // Pre-process cleanup (HTML tags) on MAIN content
    let processed_text = main_content.to_string();
    let cleaned_text = processed_text.replace("<ul>", "").replace("</ul>", "")
                          .replace("<li>", "- ").replace("</li>", "\n")
                          .replace("<br>", "\n");

    let iter = cleaned_text.lines();
    
    for line in iter {
        // Toggle Code Block
        if line.trim().starts_with("```") {
            in_code_block = !in_code_block;
            lines.push(Line::from(Span::styled(
                line.to_string(), 
                Style::default().fg(Color::DarkGray)
            )));
            continue;
        }

        if in_code_block {
            let wrapped = wrap_text(line, max_width, 0);
            for w in wrapped {
                lines.push(Line::from(Span::styled(w, Style::default().fg(Color::Yellow))));
            }
            continue;
        }

        // Table Handling - lines starting with | are table rows
        // We're flexible about trailing | since LLMs sometimes omit it
        let trimmed_line = line.trim();
        if trimmed_line.starts_with('|') {
            // Normalize: ensure it ends with | for proper parsing
            let normalized = if trimmed_line.ends_with('|') {
                line.to_string()
            } else {
                format!("{} |", line.trim())
            };
            table_lines.push(normalized);
            continue;
        } else if !table_lines.is_empty() {
            // Check if this looks like a continuation of table cell content
            // (starts with - or • or is indented, which could be cell content continuation)
            let is_continuation = trimmed_line.starts_with('-') 
                || trimmed_line.starts_with('•')
                || trimmed_line.starts_with('*')
                || (line.starts_with(' ') && !trimmed_line.is_empty() && !trimmed_line.starts_with('#'));
            
            if is_continuation {
                // Append to the last cell of the last table row
                if let Some(last_row) = table_lines.last_mut() {
                    // Remove trailing | and append content, then re-add |
                    let mut row = last_row.trim_end_matches('|').trim().to_string();
                    row.push_str(" ");
                    row.push_str(trimmed_line);
                    row.push_str(" |");
                    *last_row = row;
                }
                continue;
            }
            
            let rendered_table = render_table(&table_lines, max_width);
            lines.extend(rendered_table);
            table_lines.clear();
        }

        // Headers
        if line.trim().starts_with('#') {
            let _level = line.chars().take_while(|c| *c == '#').count();
            let content = line.trim_start_matches('#').trim();
            lines.push(Line::from("")); // Spacing
            lines.push(Line::from(Span::styled(
                content.to_string(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(Modifier::UNDERLINED)
            )));
            lines.push(Line::from(""));
            continue;
        }
        
        // Separators
        if line.trim() == "---" {
             lines.push(Line::from(Span::styled(
                "─".repeat(max_width.min(40)), 
                Style::default().fg(Color::DarkGray)
            )));
            continue;
        }

        // Standard Text (Handle lists and wrapping)
        let trimmed = line.trim_start();
        let indent = if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
             2
        } else if trimmed.chars().next().map(|c| c.is_digit(10)).unwrap_or(false) && trimmed.contains(". ") {
             3 
        } else {
             0
        };
        
        let wrapped = wrap_text(line, max_width, indent);
        for w_line in wrapped {
            lines.push(parse_inline_styles(&w_line));
        }
    }

    // Flush remaining table
    if !table_lines.is_empty() {
        let rendered_table = render_table(&table_lines, max_width);
        lines.extend(rendered_table);
    }
    
    lines
}

fn render_table(rows: &[String], max_width: usize) -> Vec<Line<'static>> {
    let parsed_rows: Vec<Vec<String>> = rows.iter().map(|row| {
        row.trim().trim_matches('|').split('|').map(|s| s.trim().to_string()).collect()
    }).collect();

    if parsed_rows.is_empty() { return Vec::new(); }

    let num_cols = parsed_rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if num_cols == 0 { return Vec::new(); }
    
    // Detect alignment from separator row (e.g., :---|:---:|---:)
    // Also find which row is the separator to identify header
    let mut alignments: Vec<Alignment> = vec![Alignment::Left; num_cols];
    let mut separator_idx: Option<usize> = None;
    
    for (idx, row) in parsed_rows.iter().enumerate() {
        if row.iter().any(|c| c.chars().all(|ch| ch == '-' || ch == ':' || ch == ' ')) 
           && row.iter().any(|c| c.contains('-')) {
            separator_idx = Some(idx);
            // Parse alignments
            for (i, cell) in row.iter().enumerate() {
                if i < num_cols {
                    let trimmed = cell.trim();
                    let starts_colon = trimmed.starts_with(':');
                    let ends_colon = trimmed.ends_with(':');
                    alignments[i] = if starts_colon && ends_colon {
                        Alignment::Center
                    } else if ends_colon {
                        Alignment::Right
                    } else {
                        Alignment::Left
                    };
                }
            }
            break;
        }
    }
    
    // Calculate max content width per column (excluding separator row)
    let mut col_widths = vec![0; num_cols];
    for (idx, row) in parsed_rows.iter().enumerate() {
        if Some(idx) == separator_idx { continue; }

        for (i, cell) in row.iter().enumerate() {
            if i < num_cols {
                let cell_content = cell.replace("<br>", "\n");
                let max_line_len = cell_content.lines().map(|l| l.chars().count()).max().unwrap_or(0);
                col_widths[i] = col_widths[i].max(max_line_len);
            }
        }
    }
    
    // Ensure minimum width
    for w in &mut col_widths {
        *w = (*w).max(3);
    }
    
    // Adjust widths to fit screen
    let total_padding = num_cols * 3 + 1;
    let usable_width = max_width.saturating_sub(total_padding);
    let total_content_width: usize = col_widths.iter().sum();
    
    if total_content_width > usable_width && total_content_width > 0 {
        for w in &mut col_widths {
            *w = (*w * usable_width) / total_content_width;
            *w = (*w).max(3);
        }
    }
    
    let mut rendered = Vec::new();
    let border_style = Style::default().fg(Color::DarkGray);
    let header_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    
    // Helper to render separator line
    let render_sep = |start: &str, mid: &str, end: &str, widths: &[usize], heavy: bool| -> Line<'static> {
        let horiz = if heavy { "━" } else { "─" };
        let mut s = String::from(start);
        for (i, w) in widths.iter().enumerate() {
             s.push_str(&horiz.repeat(*w + 2));
             if i < widths.len() - 1 { s.push_str(mid); }
        }
        s.push_str(end);
        Line::from(Span::styled(s, border_style))
    };

    rendered.push(render_sep("┌", "┬", "┐", &col_widths, false));

    let mut is_header = true; // First non-separator row is header
    
    for (r_idx, row) in parsed_rows.iter().enumerate() {
        // Skip separator rows
        if Some(r_idx) == separator_idx { 
            // Render a heavy separator after header
            if r_idx > 0 {
                rendered.push(render_sep("├", "┼", "┤", &col_widths, true));
            }
            is_header = false;
            continue; 
        }

        // Prepare cells with alignment
        let mut cell_lines_per_col: Vec<Vec<Line<'static>>> = vec![vec![]; num_cols];
        let mut max_height = 0;
        
        for i in 0..num_cols {
             let cell_text = if i < row.len() { &row[i] } else { "" };
             let width = col_widths[i];
             let align = alignments[i];
             
             let processed_cell = cell_text.replace("<br>", "\n");
             let raw_lines: Vec<&str> = processed_cell.lines().collect();
             
             let mut wrapped_cell_lines = Vec::new();
             for raw_line in raw_lines {
                 let wrapped = wrap_text(raw_line, width, 0);
                 for w in wrapped {
                     let styled_line = if is_header {
                         // Apply header style
                         let mut line = parse_inline_styles(&w);
                         line.spans = line.spans.into_iter().map(|s| {
                             Span::styled(s.content, header_style.patch(s.style))
                         }).collect();
                         line
                     } else {
                         parse_inline_styles(&w)
                     };
                     wrapped_cell_lines.push((styled_line, align));
                 }
             }
             
             if wrapped_cell_lines.is_empty() {
                 wrapped_cell_lines.push((Line::from(""), align));
             }
             
             if wrapped_cell_lines.len() > max_height {
                 max_height = wrapped_cell_lines.len();
             }
             cell_lines_per_col[i] = wrapped_cell_lines.into_iter().map(|(l, _)| l).collect();
        }
        
        // Render row lines
        for h in 0..max_height {
            let mut line_spans = Vec::new();
            line_spans.push(Span::styled("│ ", border_style));
            
            for i in 0..num_cols {
                let width = col_widths[i];
                let align = alignments[i];
                let empty_line = Line::from("");
                let content_line = cell_lines_per_col[i].get(h).unwrap_or(&empty_line);
                
                let content_width = content_line.width();
                let total_padding = width.saturating_sub(content_width);
                
                // Apply alignment padding
                let (left_pad, right_pad) = match align {
                    Alignment::Left => (0, total_padding),
                    Alignment::Right => (total_padding, 0),
                    Alignment::Center => (total_padding / 2, total_padding - total_padding / 2),
                };
                
                if left_pad > 0 {
                    line_spans.push(Span::from(" ".repeat(left_pad)));
                }
                line_spans.extend(content_line.spans.clone());
                if right_pad > 0 {
                    line_spans.push(Span::from(" ".repeat(right_pad)));
                }
                
                if i < num_cols - 1 {
                    line_spans.push(Span::styled(" │ ", border_style));
                } else {
                    line_spans.push(Span::styled(" │", border_style));
                }
            }
            rendered.push(Line::from(line_spans));
        }

        // Add separator after every body row (but not after header - that's done above)
        if !is_header && r_idx < parsed_rows.len() - 1 && Some(r_idx + 1) != separator_idx {
             rendered.push(render_sep("├", "┼", "┤", &col_widths, false));
        }
        
        // After first data row, no longer header
        if separator_idx.is_none() {
            is_header = false;
        }
    }
    
    rendered.push(render_sep("└", "┴", "┘", &col_widths, false));
    rendered
}

#[derive(Clone, Copy)]
enum Alignment {
    Left,
    Center,
    Right,
}



fn parse_inline_styles(text: &str) -> Line<'static> {
    let mut spans = Vec::new();
    let mut current_segment = String::new();
    let mut style = Style::default();
    
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    
    while i < chars.len() {
        let c = chars[i];
        
        if c == '`' {
            if !current_segment.is_empty() {
                spans.push(Span::styled(current_segment.clone(), style));
                current_segment.clear();
            }
            if style.fg == Some(Color::Yellow) {
                style = Style::default();
            } else {
                style = Style::default().fg(Color::Yellow);
            }
            i += 1;
        } 
        else if c == '*' {
             if i + 1 < chars.len() && chars[i+1] == '*' {
                 // Bold
                 if !current_segment.is_empty() {
                    spans.push(Span::styled(current_segment.clone(), style));
                    current_segment.clear();
                 }
                 if style.add_modifier.contains(Modifier::BOLD) {
                     style = style.remove_modifier(Modifier::BOLD);
                 } else {
                     style = style.add_modifier(Modifier::BOLD);
                 }
                 i += 2;
             } else if i + 1 < chars.len() && chars[i+1] != ' ' {
                 // Italic (treat as dim or italic if supported, or just ignore asterisks)
                  if !current_segment.is_empty() {
                    spans.push(Span::styled(current_segment.clone(), style));
                    current_segment.clear();
                 }
                 if style.add_modifier.contains(Modifier::ITALIC) {
                     style = style.remove_modifier(Modifier::ITALIC);
                 } else {
                     style = style.add_modifier(Modifier::ITALIC);
                 }
                 i += 1;
             } else {
                 current_segment.push(c);
                 i += 1;
             }
        } 
        else {
            current_segment.push(c);
            i += 1;
        }
    }
    
    if !current_segment.is_empty() {
        spans.push(Span::styled(current_segment, style));
    }
    
    Line::from(spans)
}

fn wrap_text(text: &str, width: usize, indent: usize) -> Vec<String> {
    if text.is_empty() { return vec![String::new()]; }
    let mut lines = Vec::new();
    let indent_str = " ".repeat(indent);
    let mut current_line = indent_str.clone();
    let mut current_width = indent;
    
    // Split by spaces but preserve words
    for word in text.split_whitespace() {
        // Clean trailing punctuation if it might affect wrapping? No, just strict width
        let word_len = word.chars().count();
        
        if current_width + word_len + 1 > width {
            lines.push(current_line);
            current_line = format!("{}{}", indent_str, word);
            current_width = indent + word_len;
        } else {
            if current_width > indent { 
                current_line.push(' '); 
                current_width += 1; 
            }
            current_line.push_str(word);
            current_width += word_len;
        }
    }
    if !current_line.trim().is_empty() {
        lines.push(current_line);
    }
    lines
}
