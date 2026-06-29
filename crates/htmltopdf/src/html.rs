use std::collections::HashMap;

use crate::color::Color;

#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    pub blocks: Vec<Block>,
    pub page_style: PageStyle,
    pub table_style: TableStyle,
    pub table_columns: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub kind: BlockKind,
    pub text: String,
    pub cells: Vec<TableCell>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    Heading1,
    Heading2,
    Paragraph,
    TableHeaderRow,
    TableRow,
    TableFooterRow,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableCell {
    pub text: String,
    pub colspan: usize,
    pub style: CellStyle,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellStyle {
    pub align: Option<TextAlign>,
    pub vertical_align: Option<VerticalAlign>,
    pub bold: bool,
    pub border: bool,
    pub overflow: Option<Overflow>,
    pub font_size: Option<f32>,
    pub padding_left: Option<f32>,
    pub padding_right: Option<f32>,
    pub padding_top: Option<f32>,
    pub padding_bottom: Option<f32>,
    pub white_space: Option<WhiteSpace>,
    pub overflow_wrap: Option<OverflowWrap>,
    pub word_break: Option<WordBreak>,
    pub color: Option<Color>,
    pub background_color: Option<Color>,
}

impl Default for CellStyle {
    fn default() -> Self {
        Self {
            align: None,
            vertical_align: None,
            bold: false,
            border: false,
            overflow: None,
            font_size: None,
            padding_left: None,
            padding_right: None,
            padding_top: None,
            padding_bottom: None,
            white_space: None,
            overflow_wrap: None,
            word_break: None,
            color: None,
            background_color: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerticalAlign {
    Top,
    Middle,
    Bottom,
    Baseline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overflow {
    Visible,
    Hidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhiteSpace {
    Normal,
    NoWrap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverflowWrap {
    Normal,
    Anywhere,
    BreakWord,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WordBreak {
    Normal,
    BreakAll,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageStyle {
    pub orientation: PageOrientation,
    pub margin_top: Option<f32>,
    pub margin_right: Option<f32>,
    pub margin_bottom: Option<f32>,
    pub margin_left: Option<f32>,
}

impl Default for PageStyle {
    fn default() -> Self {
        Self {
            orientation: PageOrientation::Portrait,
            margin_top: None,
            margin_right: None,
            margin_bottom: None,
            margin_left: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageOrientation {
    Portrait,
    Landscape,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TableStyle {
    pub row_height: Option<f32>,
}

impl Default for TableStyle {
    fn default() -> Self {
        Self { row_height: None }
    }
}

pub fn parse(input: &str) -> Document {
    let page_style = parse_page_style(input);
    let table_style = parse_table_style(input);
    let table_columns = parse_table_columns(input);
    let stylesheet = parse_stylesheet(input);
    let normalized = normalize_html_text(input);
    let mut blocks = parse_table_rows(&normalized, &stylesheet);

    if blocks.is_empty() {
        let dom = crate::dom::Dom::parse(input);
        blocks = blocks_from_dom(&dom);
    }

    Document {
        blocks,
        page_style,
        table_style,
        table_columns,
    }
}

/// Extract generic flow content (headings, paragraphs, lists) from the real DOM.
///
/// This replaces the former hand-rolled character scanner. Block-level elements
/// create block boundaries; inline elements contribute their text to the
/// enclosing block. The table path (`parse_table_rows`) still runs first; this
/// is the fallback for non-table documents and will itself move fully onto the
/// DOM as the cascade lands (ADR 0002).
fn blocks_from_dom(dom: &crate::dom::Dom) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut current = String::new();
    let mut current_kind = BlockKind::Paragraph;

    visit_block_node(dom, dom.root(), &mut blocks, &mut current_kind, &mut current);
    push_text(&mut blocks, current_kind, &mut current);

    blocks
}

fn visit_block_node(
    dom: &crate::dom::Dom,
    id: crate::dom::NodeId,
    blocks: &mut Vec<Block>,
    current_kind: &mut BlockKind,
    current: &mut String,
) {
    use crate::dom::NodeData;

    let node = dom.node(id);
    match &node.data {
        NodeData::Text(text) => current.push_str(text),
        NodeData::Document => {
            for &child in &node.children {
                visit_block_node(dom, child, blocks, current_kind, current);
            }
        }
        NodeData::Element { name, .. } => {
            // Non-rendered subtrees contribute no flow text.
            if matches!(name.as_str(), "head" | "script" | "style") {
                return;
            }

            *current_kind = handle_tag(name, blocks, *current_kind, current);
            for &child in &node.children {
                visit_block_node(dom, child, blocks, current_kind, current);
            }
            *current_kind = handle_tag(&format!("/{name}"), blocks, *current_kind, current);
        }
    }
}

fn handle_tag(
    raw_tag: &str,
    blocks: &mut Vec<Block>,
    current_kind: BlockKind,
    current: &mut String,
) -> BlockKind {
    let trimmed = raw_tag.trim();
    let is_closing = trimmed.starts_with('/');
    let tag = trimmed
        .trim_start_matches('/')
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();

    match tag.as_str() {
        "h1" => block_boundary(
            blocks,
            current_kind,
            current,
            BlockKind::Heading1,
            is_closing,
        ),
        "h2" => block_boundary(
            blocks,
            current_kind,
            current,
            BlockKind::Heading2,
            is_closing,
        ),
        "p" | "div" | "section" | "article" | "main" | "header" | "footer" => block_boundary(
            blocks,
            current_kind,
            current,
            BlockKind::Paragraph,
            is_closing,
        ),
        "br" => {
            push_text(blocks, current_kind, current);
            current_kind
        }
        "li" => {
            if is_closing {
                push_text(blocks, current_kind, current);
            } else if current.trim().is_empty() {
                current.push_str("- ");
            }
            BlockKind::Paragraph
        }
        _ => current_kind,
    }
}

fn block_boundary(
    blocks: &mut Vec<Block>,
    current_kind: BlockKind,
    current: &mut String,
    next_kind: BlockKind,
    is_closing: bool,
) -> BlockKind {
    push_text(blocks, current_kind, current);

    if is_closing {
        BlockKind::Paragraph
    } else {
        next_kind
    }
}

fn push_text(blocks: &mut Vec<Block>, kind: BlockKind, current: &mut String) {
    let text = collapse_whitespace(current);
    current.clear();

    if text.is_empty() {
        return;
    }

    blocks.push(Block {
        kind,
        text,
        cells: Vec::new(),
    });
}

fn normalize_html_text(input: &str) -> String {
    let without_scripts = strip_element(input, "script");
    let without_styles = strip_element(&without_scripts, "style");

    decode_entities(&without_styles)
}

fn strip_element(input: &str, element: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0;
    let lower = input.to_ascii_lowercase();
    let start_tag = format!("<{element}");
    let end_tag = format!("</{element}>");

    while let Some(relative_start) = lower[cursor..].find(&start_tag) {
        let start = cursor + relative_start;
        output.push_str(&input[cursor..start]);

        let Some(relative_end) = lower[start..].find(&end_tag) else {
            return output;
        };

        cursor = start + relative_end + end_tag.len();
    }

    output.push_str(&input[cursor..]);
    output
}

fn decode_entities(input: &str) -> String {
    input
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

fn parse_table_columns(input: &str) -> Vec<f32> {
    let mut columns = Vec::new();
    let mut cursor = 0;
    let lower = input.to_ascii_lowercase();

    while let Some(relative_start) = lower[cursor..].find("table.sheet0 col.col") {
        let start = cursor + relative_start;
        let Some(relative_end) = lower[start..].find('}') else {
            break;
        };
        let rule = &input[start..start + relative_end];

        if let Some(width) = parse_css_number_after(rule, "width:") {
            columns.push(width);
        }

        cursor = start + relative_end + 1;
    }

    columns
}

fn parse_page_style(input: &str) -> PageStyle {
    let lower = input.to_ascii_lowercase();
    let mut style = PageStyle::default();

    if lower.contains("size: landscape") {
        style.orientation = PageOrientation::Landscape;
    }

    if let Some(rule) = find_css_rule(input, "@page") {
        style.margin_top = parse_css_length_after(rule, "margin-top:");
        style.margin_right = parse_css_length_after(rule, "margin-right:");
        style.margin_bottom = parse_css_length_after(rule, "margin-bottom:");
        style.margin_left = parse_css_length_after(rule, "margin-left:");
    }

    style
}

fn parse_table_style(input: &str) -> TableStyle {
    let mut style = TableStyle::default();

    if let Some(rule) = find_css_rule(input, "table.sheet0 tr") {
        style.row_height = parse_css_length_after(rule, "height:");
    }

    style
}

fn find_css_rule<'a>(input: &'a str, selector: &str) -> Option<&'a str> {
    let lower = input.to_ascii_lowercase();
    let selector = selector.to_ascii_lowercase();
    let start = lower.find(&selector)?;
    let open = lower[start..].find('{').map(|position| start + position)?;
    let close = lower[open..].find('}').map(|position| open + position)?;

    Some(&input[open + 1..close])
}

fn parse_table_rows(input: &str, stylesheet: &Stylesheet) -> Vec<Block> {
    let lower = input.to_ascii_lowercase();
    if !lower.contains("<table") {
        return Vec::new();
    }

    let mut rows = Vec::new();
    let mut cursor = 0;
    let mut section = TableSection::Body;
    let mut cell_style_cache = HashMap::new();

    while let Some(relative_start) = lower[cursor..].find("<tr") {
        let row_start = cursor + relative_start;
        update_table_section(&lower[cursor..row_start], &mut section, stylesheet);

        let Some(relative_open_end) = lower[row_start..].find('>') else {
            break;
        };
        let open_tag = &input[row_start + 1..row_start + relative_open_end];
        let row_content_start = row_start + relative_open_end + 1;
        let Some(relative_row_end) = lower[row_content_start..].find("</tr>") else {
            break;
        };
        let row_content_end = row_content_start + relative_row_end;
        let row_html = &input[row_content_start..row_content_end];
        let cells = parse_table_cells(row_html, stylesheet, &mut cell_style_cache);

        if !cells.is_empty() {
            rows.push(Block {
                kind: table_row_kind(section, open_tag, stylesheet),
                text: String::new(),
                cells,
            });
        }

        cursor = row_content_end + "</tr>".len();
    }

    rows
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TableSection {
    Header,
    Body,
    Footer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CssDisplay {
    TableHeaderGroup,
    TableRowGroup,
    TableFooterGroup,
}

fn update_table_section(segment: &str, section: &mut TableSection, stylesheet: &Stylesheet) {
    let mut cursor = 0;

    while let Some(relative_start) = segment[cursor..].find('<') {
        let start = cursor + relative_start;
        let Some(relative_end) = segment[start..].find('>') else {
            break;
        };
        let raw_tag = &segment[start + 1..start + relative_end];
        let tag = raw_tag
            .trim()
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();

        let next_section = match tag.as_str() {
            "thead" => Some(TableSection::Header),
            "/thead" => {
                *section = TableSection::Body;
                None
            }
            "tbody" => Some(TableSection::Body),
            "/tbody" => {
                *section = TableSection::Body;
                None
            }
            "tfoot" => Some(TableSection::Footer),
            "/tfoot" => {
                *section = TableSection::Body;
                None
            }
            _ => None,
        };

        if let Some(default_section) = next_section {
            *section = display_to_table_section(display_for_open_tag(raw_tag, stylesheet))
                .unwrap_or(default_section);
        }

        cursor = start + relative_end + 1;
    }
}

fn table_row_kind(section: TableSection, open_tag: &str, stylesheet: &Stylesheet) -> BlockKind {
    if let Some(display_section) =
        display_to_table_section(display_for_open_tag(open_tag, stylesheet))
    {
        return match display_section {
            TableSection::Header => BlockKind::TableHeaderRow,
            TableSection::Body => BlockKind::TableRow,
            TableSection::Footer => BlockKind::TableFooterRow,
        };
    }

    match section {
        TableSection::Header => BlockKind::TableHeaderRow,
        TableSection::Body => BlockKind::TableRow,
        TableSection::Footer => BlockKind::TableFooterRow,
    }
}

fn display_to_table_section(display: Option<CssDisplay>) -> Option<TableSection> {
    match display? {
        CssDisplay::TableHeaderGroup => Some(TableSection::Header),
        CssDisplay::TableRowGroup => Some(TableSection::Body),
        CssDisplay::TableFooterGroup => Some(TableSection::Footer),
    }
}

fn display_for_open_tag(open_tag: &str, stylesheet: &Stylesheet) -> Option<CssDisplay> {
    let tag = tag_name_from_open_tag(open_tag);
    let class_attr = parse_attr(open_tag, "class").unwrap_or_default();
    let classes = class_attr.split_whitespace().collect::<Vec<_>>();
    let inline_style = parse_attr(open_tag, "style").unwrap_or_default();
    let mut declarations = stylesheet.computed_declarations(&tag, &classes);

    if !inline_style.is_empty() {
        declarations.merge_inline(parse_style_declarations(&inline_style));
    }

    declarations.resolved().display
}

fn parse_table_cells(
    row_html: &str,
    stylesheet: &Stylesheet,
    style_cache: &mut HashMap<String, CellStyle>,
) -> Vec<TableCell> {
    let lower = row_html.to_ascii_lowercase();
    let mut cursor = 0;
    let mut cells = Vec::new();

    while let Some((relative_start, tag_name)) = find_next_cell_tag(&lower[cursor..]) {
        let cell_start = cursor + relative_start;
        let Some(relative_open_end) = lower[cell_start..].find('>') else {
            break;
        };

        let open_tag = &row_html[cell_start + 1..cell_start + relative_open_end];
        let content_start = cell_start + relative_open_end + 1;
        let close_tag = format!("</{tag_name}>");
        let Some(relative_cell_end) = lower[content_start..].find(&close_tag) else {
            break;
        };
        let content_end = content_start + relative_cell_end;
        let raw_text = strip_tags(&row_html[content_start..content_end]);
        let text = collapse_whitespace(&decode_entities(&raw_text));

        cells.push(TableCell {
            text,
            colspan: parse_usize_attr(open_tag, "colspan").unwrap_or(1).max(1),
            style: cached_cell_style(open_tag, stylesheet, style_cache),
        });

        cursor = content_end + close_tag.len();
    }

    cells
}

fn cached_cell_style(
    open_tag: &str,
    stylesheet: &Stylesheet,
    cache: &mut HashMap<String, CellStyle>,
) -> CellStyle {
    let key = open_tag.trim().to_string();

    if let Some(style) = cache.get(&key) {
        return *style;
    }

    let style = parse_cell_style(open_tag, stylesheet);
    cache.insert(key, style);
    style
}

fn find_next_cell_tag(input: &str) -> Option<(usize, &'static str)> {
    let td = input.find("<td");
    let th = input.find("<th");

    match (td, th) {
        (Some(td), Some(th)) if td <= th => Some((td, "td")),
        (Some(_), Some(th)) => Some((th, "th")),
        (Some(td), None) => Some((td, "td")),
        (None, Some(th)) => Some((th, "th")),
        (None, None) => None,
    }
}

fn parse_cell_style(open_tag: &str, stylesheet: &Stylesheet) -> CellStyle {
    let tag = tag_name_from_open_tag(open_tag);
    let class_attr = parse_attr(open_tag, "class").unwrap_or_default();
    let inline_style = parse_attr(open_tag, "style").unwrap_or_default();
    let classes = class_attr.split_whitespace().collect::<Vec<_>>();

    let mut declarations = stylesheet.computed_declarations(&tag, &classes);
    let mut style = declarations.resolved().cell;

    if style.align.is_none() {
        if classes.iter().any(|class| matches!(*class, "n" | "f")) {
            style.align = Some(TextAlign::Right);
        } else if classes.iter().any(|class| matches!(*class, "b" | "e")) {
            style.align = Some(TextAlign::Center);
        }
    }

    if !inline_style.is_empty() {
        declarations = StyleDeclarations::default();
        declarations.normal.cell = style;
        declarations.merge_inline(parse_style_declarations(&inline_style));
        style = declarations.resolved().cell;
    }

    style
}

fn strip_tags(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut in_tag = false;

    for ch in input.chars() {
        match (in_tag, ch) {
            (false, '<') => in_tag = true,
            (true, '>') => in_tag = false,
            (false, _) => output.push(ch),
            (true, _) => {}
        }
    }

    output
}

fn parse_attr(open_tag: &str, attr: &str) -> Option<String> {
    let lower = open_tag.to_ascii_lowercase();
    let needle = format!("{attr}=");
    let start = lower.find(&needle)? + needle.len();
    let bytes = open_tag.as_bytes();
    let quote = *bytes.get(start)?;

    if quote != b'\'' && quote != b'"' {
        return None;
    }

    let value_start = start + 1;
    let value_end = bytes[value_start..]
        .iter()
        .position(|byte| *byte == quote)
        .map(|position| value_start + position)?;

    Some(open_tag[value_start..value_end].to_string())
}

fn parse_usize_attr(open_tag: &str, attr: &str) -> Option<usize> {
    parse_attr(open_tag, attr)?.parse().ok()
}

fn tag_name_from_open_tag(open_tag: &str) -> String {
    open_tag
        .trim()
        .trim_start_matches('/')
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn parse_css_number_after(input: &str, marker: &str) -> Option<f32> {
    let lower = input.to_ascii_lowercase();
    let start = lower.find(marker)? + marker.len();
    let value = input[start..]
        .trim_start()
        .chars()
        .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
        .collect::<String>();

    value.parse().ok()
}

fn parse_css_length_after(input: &str, marker: &str) -> Option<f32> {
    let lower = input.to_ascii_lowercase();
    let start = lower.find(marker)? + marker.len();
    let value = input[start..]
        .trim_start()
        .chars()
        .take_while(|ch| ch.is_ascii_digit() || *ch == '.' || ch.is_ascii_alphabetic())
        .collect::<String>();

    parse_css_length(&value)
}

fn parse_css_length(value: &str) -> Option<f32> {
    let number = value
        .chars()
        .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
        .collect::<String>()
        .parse::<f32>()
        .ok()?;
    let unit = value
        .chars()
        .skip_while(|ch| ch.is_ascii_digit() || *ch == '.')
        .collect::<String>()
        .to_ascii_lowercase();

    match unit.as_str() {
        "" | "pt" => Some(number),
        "in" => Some(number * 72.0),
        "px" => Some(number * 0.75),
        "cm" => Some(number * 72.0 / 2.54),
        "mm" => Some(number * 72.0 / 25.4),
        _ => None,
    }
}

#[derive(Debug, Default)]
struct Stylesheet {
    rules: Vec<StyleRule>,
    tag_rules: HashMap<String, Vec<usize>>,
    class_rules: HashMap<String, Vec<usize>>,
}

#[derive(Debug, Clone)]
struct StyleRule {
    selector: SimpleSelector,
    declarations: StyleDeclarations,
    specificity: Specificity,
    order: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SimpleSelector {
    tag: Option<String>,
    classes: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default)]
struct StyleDeclarations {
    normal: DeclarationLayer,
    important: DeclarationLayer,
}

#[derive(Debug, Clone, Copy, Default)]
struct DeclarationLayer {
    cell: CellStyle,
    display: Option<CssDisplay>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Specificity {
    ids: usize,
    classes: usize,
    elements: usize,
}

impl Stylesheet {
    fn build_indexes(&mut self) {
        self.tag_rules.clear();
        self.class_rules.clear();

        for (index, rule) in self.rules.iter().enumerate() {
            if let Some(tag) = &rule.selector.tag {
                self.tag_rules.entry(tag.clone()).or_default().push(index);
            }

            for class in &rule.selector.classes {
                self.class_rules
                    .entry(class.clone())
                    .or_default()
                    .push(index);
            }
        }
    }

    fn computed_declarations(&self, tag: &str, classes: &[&str]) -> StyleDeclarations {
        let mut candidate_indexes: Vec<usize> = Vec::new();

        if let Some(indexes) = self.tag_rules.get(tag) {
            candidate_indexes.extend(indexes.iter().copied());
        }

        for class in classes {
            if let Some(indexes) = self.class_rules.get(*class) {
                candidate_indexes.extend(indexes.iter().copied());
            }
        }

        if candidate_indexes.is_empty() {
            return StyleDeclarations::default();
        }

        candidate_indexes.sort_unstable();
        candidate_indexes.dedup();

        let mut matched = candidate_indexes
            .into_iter()
            .map(|index| &self.rules[index])
            .filter(|rule| rule.selector.matches(tag, classes))
            .collect::<Vec<_>>();

        matched.sort_by_key(|rule| (rule.specificity, rule.order));

        let mut declarations = StyleDeclarations::default();
        for rule in matched {
            declarations.merge(rule.declarations);
        }

        declarations
    }
}

impl SimpleSelector {
    fn matches(&self, tag: &str, classes: &[&str]) -> bool {
        if let Some(selector_tag) = &self.tag {
            if selector_tag != tag {
                return false;
            }
        }

        self.classes
            .iter()
            .all(|class| classes.iter().any(|candidate| candidate == class))
    }

    fn specificity(&self) -> Specificity {
        Specificity {
            ids: 0,
            classes: self.classes.len(),
            elements: usize::from(self.tag.is_some()),
        }
    }
}

impl StyleDeclarations {
    fn merge(&mut self, other: StyleDeclarations) {
        self.normal.merge(other.normal);
        self.important.merge(other.important);
    }

    fn merge_inline(&mut self, other: StyleDeclarations) {
        self.normal.merge(other.normal);
        self.important.merge(other.important);
    }

    fn resolved(&self) -> DeclarationLayer {
        let mut resolved = self.normal;
        resolved.merge(self.important);
        resolved
    }
}

impl DeclarationLayer {
    fn merge(&mut self, other: DeclarationLayer) {
        self.cell.merge(other.cell);
        self.display = other.display.or(self.display);
    }
}

fn parse_stylesheet(input: &str) -> Stylesheet {
    let mut stylesheet = Stylesheet::default();
    let mut order = 0;

    for css in extract_style_blocks(input) {
        parse_css_rules(css, &mut order, &mut stylesheet.rules);
    }

    stylesheet.build_indexes();

    stylesheet
}

fn extract_style_blocks(input: &str) -> Vec<&str> {
    let mut blocks = Vec::new();
    let lower = input.to_ascii_lowercase();
    let mut cursor = 0;

    while let Some(relative_style_start) = lower[cursor..].find("<style") {
        let style_start = cursor + relative_style_start;
        let Some(relative_open_end) = lower[style_start..].find('>') else {
            break;
        };
        let css_start = style_start + relative_open_end + 1;
        let Some(relative_style_end) = lower[css_start..].find("</style>") else {
            break;
        };
        let css_end = css_start + relative_style_end;
        blocks.push(&input[css_start..css_end]);
        cursor = css_end + "</style>".len();
    }

    blocks
}

fn parse_css_rules(css: &str, order: &mut usize, rules: &mut Vec<StyleRule>) {
    let mut cursor = 0;

    while let Some(relative_open) = css[cursor..].find('{') {
        let open = cursor + relative_open;
        let Some(close) = find_matching_brace(css, open) else {
            break;
        };
        let selectors = &css[cursor..open];
        let declarations = &css[open + 1..close];
        let trimmed_selectors = selectors.trim();

        if trimmed_selectors.starts_with("@media") {
            parse_css_rules(declarations, order, rules);
            cursor = close + 1;
            continue;
        }

        if trimmed_selectors.starts_with('@') {
            cursor = close + 1;
            continue;
        }

        let declarations = parse_style_declarations(declarations);

        for selector in parse_simple_selectors(selectors) {
            rules.push(StyleRule {
                specificity: selector.specificity(),
                selector,
                declarations,
                order: *order,
            });
            *order += 1;
        }

        cursor = close + 1;
    }
}

fn find_matching_brace(input: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;

    for (offset, ch) in input[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(open + offset);
                }
            }
            _ => {}
        }
    }

    None
}

fn parse_style_declarations(declarations: &str) -> StyleDeclarations {
    let mut parsed = StyleDeclarations::default();

    for declaration in declarations.split(';') {
        let Some((property, value)) = declaration.split_once(':') else {
            continue;
        };
        let property = property.trim().to_ascii_lowercase();
        let (value, important) = normalize_declaration_value(value);
        let target = if important {
            &mut parsed.important
        } else {
            &mut parsed.normal
        };

        apply_style_declaration(target, &property, &value);
    }

    parsed
}

fn parse_simple_selectors(selectors: &str) -> Vec<SimpleSelector> {
    let mut parsed = Vec::new();

    for selector in selectors.split(',') {
        if let Some(selector) = parse_simple_selector(selector) {
            parsed.push(selector);
        }
    }

    parsed
}

fn parse_simple_selector(selector: &str) -> Option<SimpleSelector> {
    let compound = selector
        .split(|ch: char| ch.is_whitespace() || matches!(ch, '>' | '+' | '~'))
        .filter(|part| !part.is_empty())
        .next_back()?
        .trim();

    if compound.is_empty() || compound == "*" || compound.contains('#') {
        return None;
    }

    let mut tag = None;
    let mut classes = Vec::new();
    let mut cursor = 0;
    let chars = compound.chars().collect::<Vec<_>>();

    if chars.first().is_some_and(|ch| ch.is_ascii_alphabetic()) {
        let tag_end = chars
            .iter()
            .position(|ch| !is_identifier_char(*ch))
            .unwrap_or(chars.len());
        tag = Some(
            chars[..tag_end]
                .iter()
                .collect::<String>()
                .to_ascii_lowercase(),
        );
        cursor = tag_end;
    }

    while cursor < chars.len() {
        match chars[cursor] {
            '.' => {
                cursor += 1;
                let class_start = cursor;
                while cursor < chars.len() && is_identifier_char(chars[cursor]) {
                    cursor += 1;
                }

                if class_start == cursor {
                    return None;
                }

                classes.push(chars[class_start..cursor].iter().collect::<String>());
            }
            ':' | '[' => break,
            _ => return None,
        }
    }

    if tag.is_none() && classes.is_empty() {
        return None;
    }

    Some(SimpleSelector { tag, classes })
}

fn is_identifier_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'
}

fn normalize_declaration_value(value: &str) -> (String, bool) {
    let trimmed = value.trim();
    let lower = trimmed.to_ascii_lowercase();
    let important = lower.ends_with("!important");
    let value = if important {
        trimmed[..trimmed.len() - "!important".len()].trim()
    } else {
        trimmed
    };

    (value.to_string(), important)
}

fn apply_style_declaration(target: &mut DeclarationLayer, property: &str, value: &str) {
    match property {
        "display" if value.eq_ignore_ascii_case("table-header-group") => {
            target.display = Some(CssDisplay::TableHeaderGroup);
        }
        "display" if value.eq_ignore_ascii_case("table-row-group") => {
            target.display = Some(CssDisplay::TableRowGroup);
        }
        "display" if value.eq_ignore_ascii_case("table-footer-group") => {
            target.display = Some(CssDisplay::TableFooterGroup);
        }
        "text-align" if value.eq_ignore_ascii_case("right") => {
            target.cell.align = Some(TextAlign::Right);
        }
        "text-align" if value.eq_ignore_ascii_case("center") => {
            target.cell.align = Some(TextAlign::Center);
        }
        "text-align" if value.eq_ignore_ascii_case("left") => {
            target.cell.align = Some(TextAlign::Left);
        }
        "vertical-align" if value.eq_ignore_ascii_case("top") => {
            target.cell.vertical_align = Some(VerticalAlign::Top);
        }
        "vertical-align" if value.eq_ignore_ascii_case("middle") => {
            target.cell.vertical_align = Some(VerticalAlign::Middle);
        }
        "vertical-align" if value.eq_ignore_ascii_case("bottom") => {
            target.cell.vertical_align = Some(VerticalAlign::Bottom);
        }
        "vertical-align" if value.eq_ignore_ascii_case("baseline") => {
            target.cell.vertical_align = Some(VerticalAlign::Baseline);
        }
        "font-weight" if value.eq_ignore_ascii_case("bold") || value == "700" => {
            target.cell.bold = true;
        }
        "font-size" => target.cell.font_size = parse_css_length(value),
        "color" => target.cell.color = parse_css_color(value),
        "background-color" => target.cell.background_color = parse_css_color(value),
        "background" => target.cell.background_color = parse_css_background_color(value),
        "padding-left" => target.cell.padding_left = parse_css_length(value),
        "padding-right" => target.cell.padding_right = parse_css_length(value),
        "padding-top" => target.cell.padding_top = parse_css_length(value),
        "padding-bottom" => target.cell.padding_bottom = parse_css_length(value),
        "overflow" if value.eq_ignore_ascii_case("visible") => {
            target.cell.overflow = Some(Overflow::Visible);
        }
        "overflow"
            if value.eq_ignore_ascii_case("hidden") || value.eq_ignore_ascii_case("clip") =>
        {
            target.cell.overflow = Some(Overflow::Hidden);
        }
        "white-space" if value.eq_ignore_ascii_case("nowrap") => {
            target.cell.white_space = Some(WhiteSpace::NoWrap);
        }
        "white-space" if value.eq_ignore_ascii_case("normal") => {
            target.cell.white_space = Some(WhiteSpace::Normal);
        }
        "overflow-wrap" | "word-wrap" if value.eq_ignore_ascii_case("normal") => {
            target.cell.overflow_wrap = Some(OverflowWrap::Normal);
        }
        "overflow-wrap" | "word-wrap" if value.eq_ignore_ascii_case("anywhere") => {
            target.cell.overflow_wrap = Some(OverflowWrap::Anywhere);
        }
        "overflow-wrap" | "word-wrap" if value.eq_ignore_ascii_case("break-word") => {
            target.cell.overflow_wrap = Some(OverflowWrap::BreakWord);
        }
        "word-break" if value.eq_ignore_ascii_case("normal") => {
            target.cell.word_break = Some(WordBreak::Normal);
        }
        "word-break" if value.eq_ignore_ascii_case("break-all") => {
            target.cell.word_break = Some(WordBreak::BreakAll);
        }
        "border" | "border-left" | "border-right" | "border-top" | "border-bottom"
            if !value.starts_with("none") =>
        {
            target.cell.border = true;
        }
        _ => {}
    }
}

fn parse_css_background_color(value: &str) -> Option<Color> {
    value.split_whitespace().find_map(parse_css_color)
}

fn parse_css_color(value: &str) -> Option<Color> {
    let value = value.trim().trim_matches('"').trim_matches('\'');

    if value.eq_ignore_ascii_case("transparent") {
        return None;
    }

    match value.to_ascii_lowercase().as_str() {
        "black" => return Some(Color::BLACK),
        "white" => return Some(Color::WHITE),
        "red" => return Some(Color::from_rgb_u8(255, 0, 0)),
        "green" => return Some(Color::from_rgb_u8(0, 128, 0)),
        "blue" => return Some(Color::from_rgb_u8(0, 0, 255)),
        "yellow" => return Some(Color::from_rgb_u8(255, 255, 0)),
        _ => {}
    }

    let Some(hex) = value.strip_prefix('#') else {
        return None;
    };

    match hex.len() {
        3 => {
            let r = expand_hex_nibble(hex.as_bytes()[0])?;
            let g = expand_hex_nibble(hex.as_bytes()[1])?;
            let b = expand_hex_nibble(hex.as_bytes()[2])?;
            Some(Color::from_rgb_u8(r, g, b))
        }
        6 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some(Color::from_rgb_u8(r, g, b))
        }
        _ => None,
    }
}

fn expand_hex_nibble(byte: u8) -> Option<u8> {
    let value = match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        b'A'..=b'F' => byte - b'A' + 10,
        _ => return None,
    };

    Some(value * 17)
}

impl CellStyle {
    fn merge(&mut self, other: CellStyle) {
        self.align = other.align.or(self.align);
        self.vertical_align = other.vertical_align.or(self.vertical_align);
        self.bold |= other.bold;
        self.border |= other.border;
        self.overflow = other.overflow.or(self.overflow);
        self.font_size = other.font_size.or(self.font_size);
        self.padding_left = other.padding_left.or(self.padding_left);
        self.padding_right = other.padding_right.or(self.padding_right);
        self.padding_top = other.padding_top.or(self.padding_top);
        self.padding_bottom = other.padding_bottom.or(self.padding_bottom);
        self.white_space = other.white_space.or(self.white_space);
        self.overflow_wrap = other.overflow_wrap.or(self.overflow_wrap);
        self.word_break = other.word_break.or(self.word_break);
        self.color = other.color.or(self.color);
        self.background_color = other.background_color.or(self.background_color);
    }
}

fn collapse_whitespace(input: &str) -> String {
    let mut output = String::new();
    let mut last_was_space = true;

    for ch in input.chars() {
        if ch.is_whitespace() {
            if !last_was_space {
                output.push(' ');
            }
            last_was_space = true;
        } else {
            output.push(ch);
            last_was_space = false;
        }
    }

    output.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::{parse, BlockKind};

    #[test]
    fn extracts_blocks() {
        let document = parse("<h1>Title</h1><p>Hello <strong>world</strong>.</p>");

        assert_eq!(document.blocks.len(), 2);
        assert_eq!(document.blocks[0].kind, BlockKind::Heading1);
        assert_eq!(document.blocks[0].text, "Title");
        assert_eq!(document.blocks[1].text, "Hello world.");
    }

    #[test]
    fn ignores_script_and_style_content() {
        let document =
            parse("<style>body{}</style><h1>Visible</h1><script>alert('hidden')</script>");

        assert_eq!(document.blocks.len(), 1);
        assert_eq!(document.blocks[0].text, "Visible");
    }

    #[test]
    fn parses_spreadsheet_table_rows() {
        let document = parse(
            r#"
            <style>@page page0 { size: landscape; } table.sheet0 col.col0 { width:30pt }</style>
            <table><tr><td class="style10 s" colspan="2">Student ID</td><td class="style12 n">9.00</td></tr></table>
            "#,
        );

        assert_eq!(
            document.page_style.orientation,
            super::PageOrientation::Landscape
        );
        assert_eq!(document.table_columns, vec![30.0]);
        assert_eq!(document.blocks.len(), 1);
        assert_eq!(document.blocks[0].kind, BlockKind::TableRow);
        assert_eq!(document.blocks[0].cells.len(), 2);
        assert_eq!(document.blocks[0].cells[0].text, "Student ID");
        assert_eq!(document.blocks[0].cells[0].colspan, 2);
    }

    #[test]
    fn preserves_table_header_section_rows() {
        let document = parse(
            r#"
            <table>
              <thead><tr><th>Name</th></tr></thead>
              <tbody><tr><td>Ada</td></tr></tbody>
            </table>
            "#,
        );

        assert_eq!(document.blocks.len(), 2);
        assert_eq!(document.blocks[0].kind, BlockKind::TableHeaderRow);
        assert_eq!(document.blocks[1].kind, BlockKind::TableRow);
    }

    #[test]
    fn preserves_css_table_header_group_rows() {
        let document = parse(
            r#"
            <style>.repeat { display: table-header-group; }</style>
            <table>
              <tbody class="repeat"><tr><th>Name</th></tr></tbody>
              <tbody><tr><td>Ada</td></tr></tbody>
            </table>
            "#,
        );

        assert_eq!(document.blocks.len(), 2);
        assert_eq!(document.blocks[0].kind, BlockKind::TableHeaderRow);
        assert_eq!(document.blocks[1].kind, BlockKind::TableRow);
    }

    #[test]
    fn preserves_tag_class_css_table_header_group_rows() {
        let document = parse(
            r#"
            <style>tbody.repeat { display: table-header-group; }</style>
            <table>
              <tbody class="repeat"><tr><th>Name</th></tr></tbody>
              <tbody><tr><td>Ada</td></tr></tbody>
            </table>
            "#,
        );

        assert_eq!(document.blocks.len(), 2);
        assert_eq!(document.blocks[0].kind, BlockKind::TableHeaderRow);
        assert_eq!(document.blocks[1].kind, BlockKind::TableRow);
    }

    #[test]
    fn preserves_inline_table_footer_group_rows() {
        let document = parse(
            r#"
            <table>
              <tbody><tr><td>Ada</td></tr></tbody>
              <tbody style="display: table-footer-group"><tr><td>Total</td></tr></tbody>
            </table>
            "#,
        );

        assert_eq!(document.blocks.len(), 2);
        assert_eq!(document.blocks[0].kind, BlockKind::TableRow);
        assert_eq!(document.blocks[1].kind, BlockKind::TableFooterRow);
    }

    #[test]
    fn parses_page_margins_and_row_height() {
        let document = parse(
            r#"
            <style>
            @page page0 { margin-left: 0.25in; margin-right: 0.25in; margin-top: 0.75in; margin-bottom: 0.75in; size: landscape; }
            table.sheet0 tr { height:15pt }
            </style>
            <p>Hello</p>
            "#,
        );

        assert_eq!(document.page_style.margin_left, Some(18.0));
        assert_eq!(document.page_style.margin_right, Some(18.0));
        assert_eq!(document.page_style.margin_top, Some(54.0));
        assert_eq!(document.page_style.margin_bottom, Some(54.0));
        assert_eq!(document.table_style.row_height, Some(15.0));
    }

    #[test]
    fn parses_cell_styles_from_css_classes() {
        let document = parse(
            r#"
            <style>
            td.style10, th.style10 { text-align:center; padding-left:5px; padding-right:5px; font-weight:bold; font-size:12pt; border-bottom:1px solid #000000 !important; }
            </style>
            <table><tr><td class="style10">Student ID</td></tr></table>
            "#,
        );
        let style = document.blocks[0].cells[0].style;

        assert_eq!(style.align, Some(super::TextAlign::Center));
        assert!(style.bold);
        assert!(style.border);
        assert_eq!(style.font_size, Some(12.0));
        assert_eq!(style.padding_left, Some(3.75));
        assert_eq!(style.padding_right, Some(3.75));
    }

    #[test]
    fn parses_cell_overflow_and_white_space() {
        let document = parse(
            r#"
            <style>
            td.clip { overflow:hidden; white-space:nowrap; overflow-wrap:break-word; word-break:break-all; }
            </style>
            <table><tr><td class="clip">abc@example.com</td></tr></table>
            "#,
        );
        let style = document.blocks[0].cells[0].style;

        assert_eq!(style.overflow, Some(super::Overflow::Hidden));
        assert_eq!(style.white_space, Some(super::WhiteSpace::NoWrap));
        assert_eq!(style.overflow_wrap, Some(super::OverflowWrap::BreakWord));
        assert_eq!(style.word_break, Some(super::WordBreak::BreakAll));
    }

    #[test]
    fn parses_cell_text_and_background_colors() {
        let document = parse(
            r#"
            <style>
            td.notice { color:#123456; background-color:#fed; }
            </style>
            <table><tr><td class="notice">Warning</td></tr></table>
            "#,
        );
        let style = document.blocks[0].cells[0].style;

        assert_eq!(
            style.color,
            Some(crate::color::Color::from_rgb_u8(0x12, 0x34, 0x56))
        );
        assert_eq!(
            style.background_color,
            Some(crate::color::Color::from_rgb_u8(0xff, 0xee, 0xdd))
        );
    }

    #[test]
    fn parses_cell_vertical_alignment() {
        let document = parse(
            r#"
            <style>
            td.middle { vertical-align:middle; }
            td.bottom { vertical-align:bottom; }
            </style>
            <table><tr><td class="middle">A</td><td class="bottom">B</td></tr></table>
            "#,
        );

        assert_eq!(
            document.blocks[0].cells[0].style.vertical_align,
            Some(super::VerticalAlign::Middle)
        );
        assert_eq!(
            document.blocks[0].cells[1].style.vertical_align,
            Some(super::VerticalAlign::Bottom)
        );
    }

    #[test]
    fn applies_css_source_order_for_same_specificity() {
        let document = parse(
            r#"
            <style>
            .amount { font-size:8pt; text-align:left; }
            .amount { font-size:10pt; text-align:right; }
            </style>
            <table><tr><td class="amount">9.00</td></tr></table>
            "#,
        );
        let style = document.blocks[0].cells[0].style;

        assert_eq!(style.font_size, Some(10.0));
        assert_eq!(style.align, Some(super::TextAlign::Right));
    }

    #[test]
    fn applies_higher_specificity_over_later_lower_specificity() {
        let document = parse(
            r#"
            <style>
            td.amount { font-size:12pt; text-align:center; }
            .amount { font-size:8pt; text-align:left; }
            </style>
            <table><tr><td class="amount">9.00</td></tr></table>
            "#,
        );
        let style = document.blocks[0].cells[0].style;

        assert_eq!(style.font_size, Some(12.0));
        assert_eq!(style.align, Some(super::TextAlign::Center));
    }

    #[test]
    fn applies_important_over_higher_specificity_normal_declaration() {
        let document = parse(
            r#"
            <style>
            .amount { font-size:12pt !important; text-align:right !important; }
            td.amount { font-size:8pt; text-align:left; }
            </style>
            <table><tr><td class="amount">9.00</td></tr></table>
            "#,
        );
        let style = document.blocks[0].cells[0].style;

        assert_eq!(style.font_size, Some(12.0));
        assert_eq!(style.align, Some(super::TextAlign::Right));
    }

    #[test]
    fn applies_specificity_among_important_declarations() {
        let document = parse(
            r#"
            <style>
            td.amount { font-size:8pt !important; text-align:center !important; }
            .amount { font-size:12pt !important; text-align:left !important; }
            </style>
            <table><tr><td class="amount">9.00</td></tr></table>
            "#,
        );
        let style = document.blocks[0].cells[0].style;

        assert_eq!(style.font_size, Some(8.0));
        assert_eq!(style.align, Some(super::TextAlign::Center));
    }
}
