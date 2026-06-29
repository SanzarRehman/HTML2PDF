use std::collections::HashMap;

use cssparser::{
    AtRuleParser, CowRcStr, DeclarationParser, ParseError, Parser, ParserInput, ParserState,
    QualifiedRuleParser, RuleBodyItemParser, RuleBodyParser, StyleSheetParser, Token,
};

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
    /// Computed style for flow blocks (headings/paragraphs). Table-row blocks
    /// carry styles on their cells instead and leave this at the default.
    pub style: CellStyle,
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
    let dom = crate::dom::Dom::parse(input);
    let stylesheet = parse_stylesheet(&collect_style_css(&dom));
    let computed = compute_inherited_styles(&dom, &stylesheet);

    let mut blocks = tables_from_dom(&dom, &stylesheet, &computed);
    if blocks.is_empty() {
        blocks = blocks_from_dom(&dom, &stylesheet, &computed);
    }

    Document {
        blocks,
        page_style,
        table_style,
        table_columns,
    }
}

/// Generate flow-content boxes (headings, paragraphs, lists) from the DOM,
/// driven by computed `display` and carrying computed style.
///
/// This is the box-generation step for non-table documents (ADR 0002 step 7):
/// block-level elements open a styled box, inline elements contribute their text
/// to the enclosing box, and `display: none` subtrees are skipped entirely.
/// Each emitted block carries the computed style of the element that opened it,
/// so generic content finally honors CSS color, font size, and text alignment.
/// A fully nested box tree with block/inline layout is the remaining follow-up.
fn blocks_from_dom(
    dom: &crate::dom::Dom,
    stylesheet: &Stylesheet,
    computed: &[CellStyle],
) -> Vec<Block> {
    let mut flow = FlowBuilder::default();
    visit_flow(dom, dom.root(), stylesheet, computed, &mut flow);
    flow.flush();
    flow.blocks
}

#[derive(Default)]
struct FlowBuilder {
    blocks: Vec<Block>,
    text: String,
    kind: FlowKind,
    style: CellStyle,
}

/// The kind of the block currently being accumulated. Defaults to a paragraph.
#[derive(Clone, Copy, Default)]
enum FlowKind {
    Heading1,
    Heading2,
    #[default]
    Paragraph,
}

impl FlowBuilder {
    /// Emit the accumulated text (if any) as a block, then clear it.
    fn flush(&mut self) {
        let text = collapse_whitespace(&self.text);
        self.text.clear();
        if text.is_empty() {
            return;
        }
        self.blocks.push(Block {
            kind: match self.kind {
                FlowKind::Heading1 => BlockKind::Heading1,
                FlowKind::Heading2 => BlockKind::Heading2,
                FlowKind::Paragraph => BlockKind::Paragraph,
            },
            text,
            cells: Vec::new(),
            style: self.style,
        });
    }

    /// Start a new block: flush the previous one and adopt this element's kind
    /// and computed style.
    fn open(&mut self, kind: FlowKind, style: CellStyle) {
        self.flush();
        self.kind = kind;
        self.style = style;
    }

    /// Close the current block, returning to the default paragraph context.
    fn close(&mut self) {
        self.flush();
        self.kind = FlowKind::Paragraph;
        self.style = CellStyle::default();
    }
}

fn visit_flow(
    dom: &crate::dom::Dom,
    id: crate::dom::NodeId,
    stylesheet: &Stylesheet,
    computed: &[CellStyle],
    flow: &mut FlowBuilder,
) {
    use crate::dom::NodeData;

    let node = dom.node(id);
    match &node.data {
        NodeData::Text(text) => flow.text.push_str(text),
        NodeData::Document => {
            for &child in &node.children {
                visit_flow(dom, child, stylesheet, computed, flow);
            }
        }
        NodeData::Element { name, .. } => {
            // Non-rendered subtrees and `display: none` contribute nothing.
            if matches!(name.as_str(), "head" | "script" | "style")
                || is_display_none(dom, id, stylesheet)
            {
                return;
            }

            let recurse = |flow: &mut FlowBuilder| {
                for &child in &node.children {
                    visit_flow(dom, child, stylesheet, computed, flow);
                }
            };

            match name.as_str() {
                "h1" => {
                    flow.open(FlowKind::Heading1, computed[id]);
                    recurse(flow);
                    flow.close();
                }
                "h2" => {
                    flow.open(FlowKind::Heading2, computed[id]);
                    recurse(flow);
                    flow.close();
                }
                "p" | "div" | "section" | "article" | "main" | "header" | "footer" => {
                    flow.open(FlowKind::Paragraph, computed[id]);
                    recurse(flow);
                    flow.close();
                }
                "li" => {
                    flow.open(FlowKind::Paragraph, computed[id]);
                    if flow.text.trim().is_empty() {
                        flow.text.push_str("- ");
                    }
                    recurse(flow);
                    flow.close();
                }
                // A line break ends the current line but keeps the block context.
                "br" => flow.flush(),
                // Inline (and structural) elements just contribute their content.
                _ => recurse(flow),
            }
        }
    }
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

/// Extract table rows and cells from the real DOM.
///
/// First computes every node's inherited style in one top-down pass, then walks
/// the tree tracking the current table section (the `<thead>` / `<tbody>` /
/// `<tfoot>` ancestor, with its CSS `display` group honored) and emits one
/// `Block` per `<tr>`. Cell styles are looked up from the precomputed table so
/// each cell carries properties inherited from its ancestors (ADR 0002 step 6).
fn tables_from_dom(
    dom: &crate::dom::Dom,
    stylesheet: &Stylesheet,
    computed: &[CellStyle],
) -> Vec<Block> {
    let mut rows = Vec::new();
    collect_table_rows(
        dom,
        dom.root(),
        TableSection::Body,
        stylesheet,
        computed,
        &mut rows,
    );
    rows
}

fn collect_table_rows(
    dom: &crate::dom::Dom,
    id: crate::dom::NodeId,
    section: TableSection,
    stylesheet: &Stylesheet,
    computed: &[CellStyle],
    rows: &mut Vec<Block>,
) {
    let node = dom.node(id);
    let mut child_section = section;

    match node.tag() {
        Some("thead") | Some("tbody") | Some("tfoot") => {
            let default = match node.tag() {
                Some("thead") => TableSection::Header,
                Some("tfoot") => TableSection::Footer,
                _ => TableSection::Body,
            };
            // A CSS `display: table-*-group` on the section element overrides the
            // tag default (matches browser computed display).
            child_section = display_to_table_section(computed_display_for_node(dom, id, stylesheet))
                .unwrap_or(default);
        }
        Some("tr") => {
            let cells = cells_from_row(dom, id, computed);
            if !cells.is_empty() {
                rows.push(Block {
                    kind: row_kind(dom, id, section, stylesheet),
                    text: String::new(),
                    cells,
                    style: CellStyle::default(),
                });
            }
        }
        _ => {}
    }

    for &child in &node.children {
        collect_table_rows(dom, child, child_section, stylesheet, computed, rows);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TableSection {
    Header,
    Body,
    Footer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CssDisplay {
    None,
    TableHeaderGroup,
    TableRowGroup,
    TableFooterGroup,
}

fn row_kind(
    dom: &crate::dom::Dom,
    id: crate::dom::NodeId,
    section: TableSection,
    stylesheet: &Stylesheet,
) -> BlockKind {
    // A `display: table-*-group` on the row itself overrides the inherited
    // section, matching the old open-tag behavior.
    let section =
        display_to_table_section(computed_display_for_node(dom, id, stylesheet)).unwrap_or(section);

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
        CssDisplay::None => None,
    }
}

fn is_display_none(dom: &crate::dom::Dom, id: crate::dom::NodeId, stylesheet: &Stylesheet) -> bool {
    computed_display_for_node(dom, id, stylesheet) == Some(CssDisplay::None)
}

fn computed_display_for_node(
    dom: &crate::dom::Dom,
    id: crate::dom::NodeId,
    stylesheet: &Stylesheet,
) -> Option<CssDisplay> {
    let node = dom.node(id);
    let tag = node.tag().unwrap_or_default();
    let classes = node.classes().collect::<Vec<_>>();
    let inline_style = node.attr("style").unwrap_or_default();
    let mut declarations = stylesheet.computed_declarations(tag, &classes);

    if !inline_style.is_empty() {
        declarations.merge_inline(parse_style_declarations(inline_style));
    }

    declarations.resolved().display
}

fn cells_from_row(
    dom: &crate::dom::Dom,
    tr_id: crate::dom::NodeId,
    computed: &[CellStyle],
) -> Vec<TableCell> {
    let mut cells = Vec::new();

    for &child in &dom.node(tr_id).children {
        let node = dom.node(child);
        if !matches!(node.tag(), Some("td") | Some("th")) {
            continue;
        }

        let mut text = String::new();
        collect_text(dom, child, &mut text);
        let text = collapse_whitespace(&text);

        let colspan = node
            .attr("colspan")
            .and_then(|value| value.trim().parse::<usize>().ok())
            .unwrap_or(1)
            .max(1);

        // The cell's computed style already includes properties inherited from
        // its ancestors; layer the spreadsheet class alignment heuristic on top
        // only where neither the cascade nor inheritance set an alignment.
        let mut style = computed[child];
        infer_cell_alignment(&mut style, &node.classes().collect::<Vec<_>>());

        cells.push(TableCell {
            text,
            colspan,
            style,
        });
    }

    cells
}

/// Concatenate all descendant text of a node. html5ever has already decoded
/// entities, so no further decoding is required.
fn collect_text(dom: &crate::dom::Dom, id: crate::dom::NodeId, out: &mut String) {
    match &dom.node(id).data {
        crate::dom::NodeData::Text(text) => out.push_str(text),
        _ => {
            for &child in &dom.node(id).children {
                collect_text(dom, child, out);
            }
        }
    }
}

/// Compute each node's inherited style in a single top-down pass.
///
/// A node's computed style takes its inheritable properties (color, font size,
/// font weight, text alignment, white-space, and wrapping) from its parent when
/// the node itself does not set them, and its non-inheritable properties
/// (borders, padding, background, overflow, vertical alignment) from its own
/// cascade only — matching CSS inheritance.
fn compute_inherited_styles(dom: &crate::dom::Dom, stylesheet: &Stylesheet) -> Vec<CellStyle> {
    let mut computed = vec![CellStyle::default(); dom.nodes.len()];
    let mut cache = HashMap::new();
    compute_inherited_node(
        dom,
        dom.root(),
        CellStyle::default(),
        stylesheet,
        &mut cache,
        &mut computed,
    );
    computed
}

fn compute_inherited_node(
    dom: &crate::dom::Dom,
    id: crate::dom::NodeId,
    inherited: CellStyle,
    stylesheet: &Stylesheet,
    cache: &mut HashMap<String, CellStyle>,
    computed: &mut [CellStyle],
) {
    let node = dom.node(id);
    let style = match &node.data {
        crate::dom::NodeData::Element { .. } => {
            inherit_style(&inherited, &element_own_style(dom, id, stylesheet, cache))
        }
        // Text and document nodes carry no cascade of their own; they simply
        // pass the inherited style through to descendants.
        _ => inherited,
    };
    computed[id] = style;

    for &child in &node.children {
        compute_inherited_node(dom, child, style, stylesheet, cache, computed);
    }
}

/// Combine a parent's computed style with an element's own cascaded style.
fn inherit_style(parent: &CellStyle, own: &CellStyle) -> CellStyle {
    CellStyle {
        // Inheritable: the element's own value wins, else the parent's.
        align: own.align.or(parent.align),
        font_size: own.font_size.or(parent.font_size),
        color: own.color.or(parent.color),
        white_space: own.white_space.or(parent.white_space),
        overflow_wrap: own.overflow_wrap.or(parent.overflow_wrap),
        word_break: own.word_break.or(parent.word_break),
        bold: own.bold || parent.bold,
        // Non-inheritable: the element's own value only.
        vertical_align: own.vertical_align,
        border: own.border,
        overflow: own.overflow,
        padding_left: own.padding_left,
        padding_right: own.padding_right,
        padding_top: own.padding_top,
        padding_bottom: own.padding_bottom,
        background_color: own.background_color,
    }
}

/// An element's own cascaded style (matched rules then inline `style`), without
/// inheritance or the spreadsheet alignment heuristic. Cached by the element's
/// (tag, class, inline) identity, which repeats heavily in spreadsheet exports.
fn element_own_style(
    dom: &crate::dom::Dom,
    id: crate::dom::NodeId,
    stylesheet: &Stylesheet,
    cache: &mut HashMap<String, CellStyle>,
) -> CellStyle {
    let node = dom.node(id);
    let tag = node.tag().unwrap_or_default();
    let class_attr = node.attr("class").unwrap_or_default();
    let inline_style = node.attr("style").unwrap_or_default();

    let key = format!("{tag}|{class_attr}|{inline_style}");
    if let Some(style) = cache.get(&key) {
        return *style;
    }

    let classes = class_attr.split_whitespace().collect::<Vec<_>>();
    let mut declarations = stylesheet.computed_declarations(tag, &classes);
    let mut style = declarations.resolved().cell;

    if !inline_style.is_empty() {
        declarations = StyleDeclarations::default();
        declarations.normal.cell = style;
        declarations.merge_inline(parse_style_declarations(inline_style));
        style = declarations.resolved().cell;
    }

    cache.insert(key, style);
    style
}

/// Spreadsheet exports encode numeric/centered cells with short class letters
/// (`n`/`f` → right, `b`/`e` → center). Apply that only where no alignment has
/// been set by the cascade or inheritance.
fn infer_cell_alignment(style: &mut CellStyle, classes: &[&str]) {
    if style.align.is_some() {
        return;
    }
    if classes.iter().any(|class| matches!(*class, "n" | "f")) {
        style.align = Some(TextAlign::Right);
    } else if classes.iter().any(|class| matches!(*class, "b" | "e")) {
        style.align = Some(TextAlign::Center);
    }
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

/// Concatenate the text content of every `<style>` element in the document.
/// Reading from the parsed DOM is robust where the old `<style>` substring scan
/// was not (attributes on the tag, commented-out tags, etc.).
fn collect_style_css(dom: &crate::dom::Dom) -> String {
    let mut css = String::new();
    for node in &dom.nodes {
        if node.tag() == Some("style") {
            for &child in &node.children {
                if let crate::dom::NodeData::Text(text) = &dom.node(child).data {
                    css.push_str(text);
                    css.push('\n');
                }
            }
        }
    }
    css
}

/// Parse a stylesheet using `cssparser`'s tokenizer for robust rule, selector
/// list, declaration, comment, string, and `@media` handling. The cascade model
/// (specificity, source order, `!important`) and the value/selector parsers are
/// reused unchanged, so output stays identical for inputs the old hand-rolled
/// tokenizer already handled while gaining correctness on the ones it did not
/// (comments anywhere, `;`/`{` inside strings or `url()`, nested blocks).
fn parse_stylesheet(css: &str) -> Stylesheet {
    let mut stylesheet = Stylesheet::default();
    let mut order = 0;

    let mut input = ParserInput::new(css);
    let mut parser = Parser::new(&mut input);
    let mut rule_parser = RuleParser;
    let mut rules = StyleSheetParser::new(&mut parser, &mut rule_parser);

    while let Some(result) = rules.next() {
        let Ok(parsed) = result else { continue };
        for (selector, declarations) in parsed {
            stylesheet.rules.push(StyleRule {
                specificity: selector.specificity(),
                selector,
                declarations,
                order,
            });
            order += 1;
        }
    }

    stylesheet.build_indexes();
    stylesheet
}

/// Parse an inline `style="..."` declaration list with `cssparser`.
fn parse_style_declarations(declarations: &str) -> StyleDeclarations {
    let mut parsed = StyleDeclarations::default();

    let mut input = ParserInput::new(declarations);
    let mut parser = Parser::new(&mut input);
    let mut decl_parser = DeclParser {
        declarations: &mut parsed,
    };
    let mut items = RuleBodyParser::new(&mut parser, &mut decl_parser);
    while let Some(result) = items.next() {
        let _ = result;
    }

    parsed
}

/// A parsed style rule's prelude is a selector list; its block is a declaration
/// list. One comma-separated rule expands into several `(selector, decls)` pairs.
type ParsedRule = (SimpleSelector, StyleDeclarations);

struct RuleParser;

enum AtRuleKind {
    /// `@media`: parse the nested block as if its rules were top-level.
    Media,
    /// Any other at-rule: ignored.
    Other,
}

impl<'i> QualifiedRuleParser<'i> for RuleParser {
    type Prelude = Vec<SimpleSelector>;
    type QualifiedRule = Vec<ParsedRule>;
    type Error = ();

    fn parse_prelude<'t>(
        &mut self,
        input: &mut Parser<'i, 't>,
    ) -> Result<Self::Prelude, ParseError<'i, ()>> {
        Ok(parse_selector_list(input))
    }

    fn parse_block<'t>(
        &mut self,
        prelude: Self::Prelude,
        _start: &ParserState,
        input: &mut Parser<'i, 't>,
    ) -> Result<Self::QualifiedRule, ParseError<'i, ()>> {
        let declarations = parse_declaration_block(input);
        Ok(prelude
            .into_iter()
            .map(|selector| (selector, declarations))
            .collect())
    }
}

impl<'i> AtRuleParser<'i> for RuleParser {
    type Prelude = AtRuleKind;
    type AtRule = Vec<ParsedRule>;
    type Error = ();

    fn parse_prelude<'t>(
        &mut self,
        name: CowRcStr<'i>,
        input: &mut Parser<'i, 't>,
    ) -> Result<Self::Prelude, ParseError<'i, ()>> {
        consume_remaining(input);
        if name.eq_ignore_ascii_case("media") {
            Ok(AtRuleKind::Media)
        } else {
            Ok(AtRuleKind::Other)
        }
    }

    fn parse_block<'t>(
        &mut self,
        prelude: Self::Prelude,
        _start: &ParserState,
        input: &mut Parser<'i, 't>,
    ) -> Result<Self::AtRule, ParseError<'i, ()>> {
        match prelude {
            AtRuleKind::Media => {
                let mut inner = RuleParser;
                let mut rules = StyleSheetParser::new(input, &mut inner);
                let mut collected = Vec::new();
                while let Some(result) = rules.next() {
                    if let Ok(mut parsed) = result {
                        collected.append(&mut parsed);
                    }
                }
                Ok(collected)
            }
            AtRuleKind::Other => Ok(Vec::new()),
        }
    }

    fn rule_without_block(
        &mut self,
        _prelude: Self::Prelude,
        _start: &ParserState,
    ) -> Result<Self::AtRule, ()> {
        // At-rules without a block (e.g. `@import`, `@charset`) contribute no
        // style rules to the cascade.
        Ok(Vec::new())
    }
}

/// Parse a declaration list (a rule's `{ ... }` body) into a `StyleDeclarations`.
fn parse_declaration_block(input: &mut Parser<'_, '_>) -> StyleDeclarations {
    let mut declarations = StyleDeclarations::default();
    let mut decl_parser = DeclParser {
        declarations: &mut declarations,
    };
    let mut items = RuleBodyParser::new(input, &mut decl_parser);
    while let Some(result) = items.next() {
        let _ = result;
    }
    declarations
}

/// Applies each declaration into the normal/important layers, reusing the
/// existing value normalization and property mapping.
struct DeclParser<'a> {
    declarations: &'a mut StyleDeclarations,
}

impl<'i> DeclarationParser<'i> for DeclParser<'_> {
    type Declaration = ();
    type Error = ();

    fn parse_value<'t>(
        &mut self,
        name: CowRcStr<'i>,
        input: &mut Parser<'i, 't>,
        _start: &ParserState,
    ) -> Result<(), ParseError<'i, ()>> {
        let value_start = input.position();
        consume_remaining(input);
        let raw_value = input.slice_from(value_start);

        let property = name.to_ascii_lowercase();
        let (value, important) = normalize_declaration_value(raw_value);
        let layer = if important {
            &mut self.declarations.important
        } else {
            &mut self.declarations.normal
        };

        apply_style_declaration(layer, &property, &value);
        Ok(())
    }
}

// Declaration lists may, per the CSS syntax spec, contain at-rules and (in
// nesting) qualified rules. We don't support those inside a block, so reject
// them with empty implementations and only opt into declaration parsing.
impl<'i> AtRuleParser<'i> for DeclParser<'_> {
    type Prelude = ();
    type AtRule = ();
    type Error = ();
}

impl<'i> QualifiedRuleParser<'i> for DeclParser<'_> {
    type Prelude = ();
    type QualifiedRule = ();
    type Error = ();
}

impl<'i> RuleBodyItemParser<'i, (), ()> for DeclParser<'_> {
    fn parse_declarations(&self) -> bool {
        true
    }

    fn parse_qualified(&self) -> bool {
        false
    }
}

/// Consume the rest of a delimited `cssparser` parser, descending into and
/// skipping any nested `{}`/`()`/`[]`/function blocks, so a following
/// `slice_from` captures the full source text of a prelude or declaration value.
fn consume_remaining(input: &mut Parser<'_, '_>) {
    while let Ok(token) = input.next_including_whitespace().map(|token| token.clone()) {
        if matches!(
            token,
            Token::Function(_)
                | Token::ParenthesisBlock
                | Token::SquareBracketBlock
                | Token::CurlyBracketBlock
        ) {
            let _ = input.parse_nested_block(|nested| -> Result<(), ParseError<'_, ()>> {
                consume_remaining(nested);
                Ok(())
            });
        }
    }
}

/// Parse a comma-separated selector list from `cssparser` tokens into the
/// engine's `SimpleSelector` model (rightmost compound: an optional type tag
/// plus class names). Tokenizing rather than string-splitting means comments
/// inside selectors are skipped and `,`/combinators inside blocks (e.g.
/// `:not(...)`, attribute selectors) do not split the list incorrectly.
///
/// Selectors using ids, the universal selector, or anything unsupported in the
/// rightmost compound are dropped, matching the prior parser. Pseudo-classes and
/// attribute selectors end the compound but keep the tag/classes parsed so far.
fn parse_selector_list<'i>(input: &mut Parser<'i, '_>) -> Vec<SimpleSelector> {
    let mut selectors = Vec::new();
    let mut current = CompoundBuilder::default();

    while let Ok(token) = input.next_including_whitespace().map(|token| token.clone()) {
        match token {
            Token::Comma => {
                current.finish_into(&mut selectors);
                current = CompoundBuilder::default();
            }
            // Combinators only separate compounds when another compound actually
            // follows. Defer the reset so trailing whitespace before `{` (or
            // before a comma) does not wipe the rightmost compound.
            Token::WhiteSpace(_)
            | Token::Delim('>')
            | Token::Delim('+')
            | Token::Delim('~') => current.pending_combinator = true,
            // Blocks (attribute selectors, functional pseudo-classes): start a
            // fresh compound if one is pending, end it, and consume the block.
            Token::Function(_)
            | Token::ParenthesisBlock
            | Token::SquareBracketBlock
            | Token::CurlyBracketBlock => {
                current.begin_compound();
                current.stop();
                let _ = input.parse_nested_block(|nested| -> Result<(), ParseError<'_, ()>> {
                    consume_remaining(nested);
                    Ok(())
                });
            }
            _ if current.stopped => {}
            Token::Delim('.') => {
                current.begin_compound();
                current.expect_class = true;
            }
            Token::Ident(name) => {
                current.begin_compound();
                current.push_ident(&name);
            }
            Token::Colon => {
                current.begin_compound();
                current.stop();
            }
            // Ids and the universal selector are unsupported: drop the selector.
            Token::IDHash(_) | Token::Hash(_) | Token::Delim('*') => {
                current.begin_compound();
                current.reject();
            }
            _ => current.reject(),
        }
    }

    current.finish_into(&mut selectors);
    selectors
}

#[derive(Default)]
struct CompoundBuilder {
    tag: Option<String>,
    classes: Vec<String>,
    rejected: bool,
    stopped: bool,
    expect_class: bool,
    pending_combinator: bool,
}

impl CompoundBuilder {
    /// Apply a deferred combinator: if one is pending, the tokens seen so far
    /// belonged to an earlier compound and only the rightmost compound matters,
    /// so discard them and begin fresh.
    fn begin_compound(&mut self) {
        if self.pending_combinator {
            *self = CompoundBuilder::default();
        }
    }

    fn stop(&mut self) {
        self.stopped = true;
        self.expect_class = false;
    }

    fn reject(&mut self) {
        self.rejected = true;
    }

    fn push_ident(&mut self, name: &str) {
        if self.expect_class {
            // Class names are case-sensitive; only type names are lowercased.
            self.classes.push(name.to_string());
            self.expect_class = false;
        } else if self.tag.is_none() && self.classes.is_empty() {
            self.tag = Some(name.to_ascii_lowercase());
        } else {
            self.reject();
        }
    }

    fn finish_into(self, out: &mut Vec<SimpleSelector>) {
        if self.rejected || (self.tag.is_none() && self.classes.is_empty()) {
            return;
        }
        out.push(SimpleSelector {
            tag: self.tag,
            classes: self.classes,
        });
    }
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
        "display" if value.eq_ignore_ascii_case("none") => {
            target.display = Some(CssDisplay::None);
        }
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
    let value = value.trim().trim_matches('"').trim_matches('\'').trim();

    if value.eq_ignore_ascii_case("transparent") {
        return None;
    }

    if let Some(hex) = value.strip_prefix('#') {
        return parse_hex_color(hex);
    }

    // Functional notation: rgb()/rgba()/hsl()/hsla(), with comma or space-
    // separated components and an optional `/ alpha` (alpha is ignored since the
    // engine renders opaque colors today).
    if let Some(open) = value.find('(') {
        let stripped = value.strip_suffix(')')?;
        let function = value[..open].trim().to_ascii_lowercase();
        let args = &stripped[open + 1..];
        return match function.as_str() {
            "rgb" | "rgba" => parse_rgb_function(args),
            "hsl" | "hsla" => parse_hsl_function(args),
            _ => None,
        };
    }

    named_color(&value.to_ascii_lowercase())
}

fn parse_hex_color(hex: &str) -> Option<Color> {
    match hex.len() {
        // #rgb and #rgba (alpha ignored).
        3 | 4 => {
            let r = expand_hex_nibble(hex.as_bytes()[0])?;
            let g = expand_hex_nibble(hex.as_bytes()[1])?;
            let b = expand_hex_nibble(hex.as_bytes()[2])?;
            Some(Color::from_rgb_u8(r, g, b))
        }
        // #rrggbb and #rrggbbaa (alpha ignored).
        6 | 8 => {
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

/// Split color-function arguments on commas, slashes (alpha separator), and
/// whitespace, so both legacy `rgb(r, g, b)` and modern `rgb(r g b / a)` work.
fn color_components(args: &str) -> Vec<&str> {
    args.split([',', '/', ' ', '\t', '\n'])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect()
}

fn parse_rgb_function(args: &str) -> Option<Color> {
    let parts = color_components(args);
    if parts.len() < 3 {
        return None;
    }
    Some(Color::from_rgb_u8(
        parse_rgb_channel(parts[0])?,
        parse_rgb_channel(parts[1])?,
        parse_rgb_channel(parts[2])?,
    ))
}

/// A single rgb() channel: either 0..=255 or a percentage.
fn parse_rgb_channel(part: &str) -> Option<u8> {
    let scaled = if let Some(percent) = part.strip_suffix('%') {
        percent.trim().parse::<f32>().ok()? / 100.0 * 255.0
    } else {
        part.parse::<f32>().ok()?
    };
    Some(scaled.round().clamp(0.0, 255.0) as u8)
}

fn parse_hsl_function(args: &str) -> Option<Color> {
    let parts = color_components(args);
    if parts.len() < 3 {
        return None;
    }
    let hue = parts[0].trim_end_matches("deg").trim().parse::<f32>().ok()?;
    let saturation = parts[1].trim_end_matches('%').trim().parse::<f32>().ok()? / 100.0;
    let lightness = parts[2].trim_end_matches('%').trim().parse::<f32>().ok()? / 100.0;
    Some(hsl_to_color(hue, saturation.clamp(0.0, 1.0), lightness.clamp(0.0, 1.0)))
}

fn hsl_to_color(hue: f32, saturation: f32, lightness: f32) -> Color {
    let hue = (hue.rem_euclid(360.0)) / 360.0;
    let (r, g, b) = if saturation == 0.0 {
        (lightness, lightness, lightness)
    } else {
        let q = if lightness < 0.5 {
            lightness * (1.0 + saturation)
        } else {
            lightness + saturation - lightness * saturation
        };
        let p = 2.0 * lightness - q;
        (
            hue_to_rgb(p, q, hue + 1.0 / 3.0),
            hue_to_rgb(p, q, hue),
            hue_to_rgb(p, q, hue - 1.0 / 3.0),
        )
    };

    // Quantize through the same 0..=255 path every other color uses, so equal
    // colors compare equal regardless of how they were written.
    let to_u8 = |channel: f32| (channel.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color::from_rgb_u8(to_u8(r), to_u8(g), to_u8(b))
}

fn hue_to_rgb(p: f32, q: f32, t: f32) -> f32 {
    let t = t.rem_euclid(1.0);
    if t < 1.0 / 6.0 {
        p + (q - p) * 6.0 * t
    } else if t < 1.0 / 2.0 {
        q
    } else if t < 2.0 / 3.0 {
        p + (q - p) * (2.0 / 3.0 - t) * 6.0
    } else {
        p
    }
}

/// A practical subset of CSS named colors. Keeps the original six mappings
/// exactly (black/white/red/green/blue/yellow) and adds common extras.
fn named_color(name: &str) -> Option<Color> {
    let (r, g, b) = match name {
        "black" => (0, 0, 0),
        "white" => (255, 255, 255),
        "red" => (255, 0, 0),
        "green" => (0, 128, 0),
        "blue" => (0, 0, 255),
        "yellow" => (255, 255, 0),
        "silver" => (192, 192, 192),
        "gray" | "grey" => (128, 128, 128),
        "maroon" => (128, 0, 0),
        "olive" => (128, 128, 0),
        "lime" => (0, 255, 0),
        "aqua" | "cyan" => (0, 255, 255),
        "teal" => (0, 128, 128),
        "navy" => (0, 0, 128),
        "fuchsia" | "magenta" => (255, 0, 255),
        "purple" => (128, 0, 128),
        "orange" => (255, 165, 0),
        "pink" => (255, 192, 203),
        "brown" => (165, 42, 42),
        "gold" => (255, 215, 0),
        "lightgray" | "lightgrey" => (211, 211, 211),
        "darkgray" | "darkgrey" => (169, 169, 169),
        "whitesmoke" => (245, 245, 245),
        "lightblue" => (173, 216, 230),
        "lightgreen" => (144, 238, 144),
        _ => return None,
    };
    Some(Color::from_rgb_u8(r, g, b))
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
    fn skips_display_none_flow_content() {
        let document = parse(
            r#"
            <style>.hidden { display: none; }</style>
            <p>shown</p>
            <p class="hidden">secret</p>
            <div style="display:none">also secret</div>
            "#,
        );

        let texts: Vec<&str> = document.blocks.iter().map(|b| b.text.as_str()).collect();
        assert_eq!(texts, vec!["shown"]);
    }

    #[test]
    fn applies_computed_style_to_flow_blocks() {
        let document = parse(
            r#"
            <style>
            p.note { color: #112233; font-size: 14pt; text-align: center; }
            </style>
            <p class="note">styled</p>
            "#,
        );
        let block = &document.blocks[0];

        assert_eq!(block.kind, BlockKind::Paragraph);
        assert_eq!(block.text, "styled");
        assert_eq!(
            block.style.color,
            Some(crate::color::Color::from_rgb_u8(0x11, 0x22, 0x33))
        );
        assert_eq!(block.style.font_size, Some(14.0));
        assert_eq!(block.style.align, Some(super::TextAlign::Center));
    }

    #[test]
    fn flow_blocks_inherit_style_from_ancestors() {
        let document = parse(
            r#"
            <style>body { color: #445566; }</style>
            <body><div><p>deep</p></div></body>
            "#,
        );

        assert_eq!(
            document.blocks[0].style.color,
            Some(crate::color::Color::from_rgb_u8(0x44, 0x55, 0x66))
        );
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
    fn parses_rgb_hsl_and_named_colors() {
        use crate::color::Color;
        // rgb() with commas and with modern space/slash syntax.
        assert_eq!(
            super::parse_css_color("rgb(18, 52, 86)"),
            Some(Color::from_rgb_u8(18, 52, 86))
        );
        assert_eq!(
            super::parse_css_color("rgba(18 52 86 / 50%)"),
            Some(Color::from_rgb_u8(18, 52, 86))
        );
        // Percentage channels.
        assert_eq!(
            super::parse_css_color("rgb(100%, 0%, 0%)"),
            Some(Color::from_rgb_u8(255, 0, 0))
        );
        // hsl(): pure red and a gray.
        assert_eq!(
            super::parse_css_color("hsl(0, 100%, 50%)"),
            Some(Color::from_rgb_u8(255, 0, 0))
        );
        assert_eq!(
            super::parse_css_color("hsl(0, 0%, 50%)"),
            Some(Color::from_rgb_u8(128, 128, 128))
        );
        // Extended named colors and 4/8-digit hex (alpha ignored).
        assert_eq!(
            super::parse_css_color("teal"),
            Some(Color::from_rgb_u8(0, 128, 128))
        );
        assert_eq!(
            super::parse_css_color("#11223344"),
            Some(Color::from_rgb_u8(0x11, 0x22, 0x33))
        );
        // Original mappings unchanged.
        assert_eq!(super::parse_css_color("red"), Some(Color::from_rgb_u8(255, 0, 0)));
        assert_eq!(super::parse_css_color("white"), Some(Color::WHITE));
        assert_eq!(super::parse_css_color("bogus"), None);
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
    fn ignores_css_comments_in_rules_and_declarations() {
        // The old hand-rolled tokenizer split on raw ';' and '{', so a comment
        // containing them would corrupt parsing. cssparser handles this.
        let document = parse(
            r#"
            <style>
            /* a stray ; and { } in a comment */
            td.amount /* comment */ {
                font-size: 10pt; /* ; ; ; */
                text-align: right;
            }
            </style>
            <table><tr><td class="amount">9.00</td></tr></table>
            "#,
        );
        let style = document.blocks[0].cells[0].style;

        assert_eq!(style.font_size, Some(10.0));
        assert_eq!(style.align, Some(super::TextAlign::Right));
    }

    #[test]
    fn handles_semicolons_inside_string_values() {
        // A ';' inside a quoted value must not end the declaration early.
        let document = parse(
            r#"
            <style>
            td.q { font-family: "Weird; Font"; text-align: center; }
            </style>
            <table><tr><td class="q">x</td></tr></table>
            "#,
        );

        assert_eq!(
            document.blocks[0].cells[0].style.align,
            Some(super::TextAlign::Center)
        );
    }

    #[test]
    fn parses_rules_inside_media_blocks() {
        let document = parse(
            r#"
            <style>
            @media print {
                td.amount { text-align: right; font-size: 12pt; }
            }
            </style>
            <table><tr><td class="amount">9.00</td></tr></table>
            "#,
        );
        let style = document.blocks[0].cells[0].style;

        assert_eq!(style.align, Some(super::TextAlign::Right));
        assert_eq!(style.font_size, Some(12.0));
    }

    #[test]
    fn inherits_color_and_font_size_from_ancestors() {
        let document = parse(
            r#"
            <style>
            table { color: #123456; font-size: 13pt; }
            </style>
            <table><tr><td>x</td></tr></table>
            "#,
        );
        let style = document.blocks[0].cells[0].style;

        assert_eq!(
            style.color,
            Some(crate::color::Color::from_rgb_u8(0x12, 0x34, 0x56))
        );
        assert_eq!(style.font_size, Some(13.0));
    }

    #[test]
    fn own_style_overrides_inherited() {
        let document = parse(
            r#"
            <style>
            table { font-size: 13pt; }
            td.big { font-size: 20pt; }
            </style>
            <table><tr><td class="big">x</td></tr></table>
            "#,
        );

        assert_eq!(document.blocks[0].cells[0].style.font_size, Some(20.0));
    }

    #[test]
    fn inherits_text_align_through_intermediate_ancestors() {
        let document = parse(
            r#"
            <style>
            table { text-align: center; }
            </style>
            <table><tbody><tr><td>x</td></tr></tbody></table>
            "#,
        );

        assert_eq!(
            document.blocks[0].cells[0].style.align,
            Some(super::TextAlign::Center)
        );
    }

    #[test]
    fn does_not_inherit_non_inheritable_properties() {
        // border and background-color must not flow from an ancestor to a cell.
        let document = parse(
            r#"
            <style>
            table { border: 1px solid black; background-color: #abcdef; }
            </style>
            <table><tr><td>x</td></tr></table>
            "#,
        );
        let style = document.blocks[0].cells[0].style;

        assert!(!style.border);
        assert_eq!(style.background_color, None);
    }

    #[test]
    fn reads_style_css_from_the_dom() {
        // Two separate <style> elements both contribute to the cascade.
        let document = parse(
            r#"
            <style>td.a { text-align: right; }</style>
            <style>td.a { font-size: 9pt; }</style>
            <table><tr><td class="a">9.00</td></tr></table>
            "#,
        );
        let style = document.blocks[0].cells[0].style;

        assert_eq!(style.align, Some(super::TextAlign::Right));
        assert_eq!(style.font_size, Some(9.0));
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
