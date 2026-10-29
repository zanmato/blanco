use std::fmt::Write as _;

use gpui::{App, Hsla, Point};
use gpui_component::ActiveTheme;

use super::{
    COMMENT_MARKER_WIDTH, GraphModel, GraphNode, GraphView, NODE_BORDER_RADIUS, NODE_HEADER_HEIGHT,
    NODE_PADDING, NODE_SUBTITLE_HEIGHT, ROUTE_CORNER_RADIUS, field_y_offset, node_header_height,
    route_edges, schema_color,
};

/// Space around the graph, so borders and routes at its edge are not cut.
const MARGIN: f32 = 24.0;
const FONT_SIZE: f32 = 13.0;
const BADGE_FONT_SIZE: f32 = 10.0;
/// A rough width of one character relative to the font size, for placing
/// what the browser cannot place itself: badges and the legend.
const CHARACTER_WIDTH: f32 = 0.6;
const LEGEND_ROW_HEIGHT: f32 = 16.0;

/// The colors of the export, read from the theme once.
struct Palette {
    canvas: Hsla,
    node: Hsla,
    header: Hsla,
    border: Hsla,
    foreground: Hsla,
    muted: Hsla,
    edge: Hsla,
}

impl GraphView {
    /// The shown graph as an SVG document: every table at its current place
    /// and every relationship, whichever are selected, in the theme's colors.
    pub fn export_svg(&self, cx: &App) -> String {
        let theme = cx.theme();
        let palette = Palette {
            canvas: theme.secondary,
            node: theme.background,
            header: theme.title_bar,
            border: theme.border,
            foreground: theme.foreground,
            muted: theme.muted_foreground,
            edge: theme.muted_foreground.opacity(0.5),
        };
        let legend: Vec<(String, Hsla)> = if self.colors_schemas() {
            self.schemas()
                .into_iter()
                .map(|schema| {
                    let color = schema_color(&schema, cx);
                    (schema.to_string(), color)
                })
                .collect()
        } else {
            Vec::new()
        };
        let header_color = |node: &GraphNode| match &node.schema {
            Some(schema) if !legend.is_empty() => {
                palette.header.blend(schema_color(schema, cx).opacity(0.3))
            }
            _ => palette.header,
        };
        let badge_colors: Vec<_> = [super::FieldBadge::PrimaryKey, super::FieldBadge::ForeignKey]
            .into_iter()
            .map(|badge| {
                let color = badge.color(cx);
                (badge, color)
            })
            .collect();
        write_svg(
            &self.model,
            &palette,
            &legend,
            &header_color,
            &|badge| {
                badge_colors
                    .iter()
                    .find(|(known, _)| known == badge)
                    .map_or(palette.muted, |(_, color)| *color)
            },
            &theme.font_family,
        )
    }
}

fn write_svg(
    model: &GraphModel,
    palette: &Palette,
    legend: &[(String, Hsla)],
    header_color: &dyn Fn(&GraphNode) -> Hsla,
    badge_color: &dyn Fn(&super::FieldBadge) -> Hsla,
    font_family: &str,
) -> String {
    // Every edge, not only the selected tables' as on screen.
    let routes = route_edges(model, |_| true, |_| false);

    let mut min = Point::new(f32::MAX, f32::MAX);
    let mut max = Point::new(f32::MIN, f32::MIN);
    let mut include = |point: Point<f32>| {
        min = Point::new(min.x.min(point.x), min.y.min(point.y));
        max = Point::new(max.x.max(point.x), max.y.max(point.y));
    };
    for node in &model.nodes {
        include(node.position);
        include(Point::new(
            node.position.x + node.size.width,
            node.position.y + node.size.height,
        ));
    }
    for route in &routes {
        route.points.iter().copied().for_each(&mut include);
    }
    if model.nodes.is_empty() {
        min = Point::default();
        max = Point::default();
    }
    let legend_height = if legend.is_empty() {
        0.0
    } else {
        MARGIN / 2.0 + legend.len() as f32 * LEGEND_ROW_HEIGHT
    };
    let legend_width = legend
        .iter()
        .map(|(schema, _)| 18.0 + schema.chars().count() as f32 * 11.0 * CHARACTER_WIDTH)
        .fold(0.0, f32::max);
    let width = (max.x - min.x).max(legend_width) + MARGIN * 2.0;
    let height = max.y - min.y + legend_height + MARGIN * 2.0;
    let offset = Point::new(MARGIN - min.x, MARGIN - min.y);
    let at = |point: Point<f32>| Point::new(point.x + offset.x, point.y + offset.y);

    let mut svg = String::new();
    // Writing into a String cannot fail.
    writeln!(
        svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width:.0}" height="{height:.0}" viewBox="0 0 {width:.0} {height:.0}" font-family="{}, sans-serif" font-size="{FONT_SIZE}">"#,
        escape(font_family)
    )
    .ok();
    writeln!(
        svg,
        r#"<rect width="100%" height="100%"{}/>"#,
        paint("fill", palette.canvas)
    )
    .ok();

    for route in &routes {
        let dashed = model
            .edges
            .get(route.edge_ix)
            .is_some_and(|edge| edge.dashed);
        let points: Vec<Point<f32>> = route.points.iter().map(|point| at(*point)).collect();
        writeln!(
            svg,
            r#"<path d="{}" fill="none"{} stroke-width="1.5"{}/>"#,
            rounded_path(&points, ROUTE_CORNER_RADIUS),
            paint("stroke", palette.edge),
            if dashed {
                r#" stroke-dasharray="6 4""#
            } else {
                ""
            }
        )
        .ok();
    }

    for node in &model.nodes {
        write_node(
            &mut svg,
            node,
            at(node.position),
            palette,
            header_color(node),
            badge_color,
        );
    }

    let legend_top = max.y + offset.y + MARGIN / 2.0;
    for (row, (schema, schema_color)) in legend.iter().enumerate() {
        let y = legend_top + row as f32 * LEGEND_ROW_HEIGHT;
        writeln!(
            svg,
            r#"<rect x="{MARGIN}" y="{:.1}" width="10" height="10" rx="2"{}/><text x="{:.1}" y="{:.1}" font-size="11" dominant-baseline="central"{}>{}</text>"#,
            y + 3.0,
            paint("fill", *schema_color),
            MARGIN + 16.0,
            y + 8.0,
            paint("fill", palette.foreground),
            escape(schema)
        )
        .ok();
    }

    svg.push_str("</svg>\n");
    svg
}

fn write_node(
    svg: &mut String,
    node: &GraphNode,
    origin: Point<f32>,
    palette: &Palette,
    header: Hsla,
    badge_color: &dyn Fn(&super::FieldBadge) -> Hsla,
) {
    let (x, y) = (origin.x, origin.y);
    let (width, height) = (node.size.width, node.size.height);
    let radius = NODE_BORDER_RADIUS;
    let header_height = node_header_height(node);

    svg.push_str("<g>");
    if let Some(comment) = &node.comment {
        write!(svg, "<title>{}</title>", escape(comment)).ok();
    }
    writeln!(
        svg,
        r#"<rect x="{x:.1}" y="{y:.1}" width="{width:.1}" height="{height:.1}" rx="{radius}"{}/>"#,
        paint("fill", palette.node)
    )
    .ok();
    // The header's top corners follow the node's, its bottom is square.
    writeln!(
        svg,
        r#"<path d="M{x:.1},{:.1} V{:.1} A{radius},{radius} 0 0 1 {:.1},{y:.1} H{:.1} A{radius},{radius} 0 0 1 {:.1},{:.1} V{:.1} Z"{}/>"#,
        y + header_height,
        y + radius,
        x + radius,
        x + width - radius,
        x + width,
        y + radius,
        y + header_height,
        paint("fill", header)
    )
    .ok();

    let text_x = x + NODE_PADDING;
    let text_width = width - NODE_PADDING * 2.0;
    let title_y = if node.comment.is_some() {
        y + (header_height - NODE_SUBTITLE_HEIGHT) / 2.0 + 2.0
    } else {
        y + NODE_HEADER_HEIGHT / 2.0
    };
    writeln!(
        svg,
        r#"<text x="{text_x:.1}" y="{title_y:.1}" dominant-baseline="central" font-weight="600"{}>{}</text>"#,
        paint("fill", palette.foreground),
        escape(&truncate(&node.title, text_width, FONT_SIZE))
    )
    .ok();
    if let Some(comment) = &node.comment {
        let size = FONT_SIZE * 0.85;
        writeln!(
            svg,
            r#"<text x="{text_x:.1}" y="{:.1}" dominant-baseline="central" font-size="{size:.2}"{}>{}</text>"#,
            title_y + NODE_SUBTITLE_HEIGHT + 1.0,
            paint("fill", palette.muted),
            escape(&truncate(comment, text_width, size))
        )
        .ok();
    }

    for (index, field) in node.fields.iter().enumerate() {
        let row_middle = y + field_y_offset(node, index);
        svg.push_str("<g>");
        if let Some(comment) = &field.comment {
            write!(svg, "<title>{}</title>", escape(comment)).ok();
        }
        // Badges sit at the right edge with the type before them, as the
        // row lays them out on screen.
        let mut right = x + width - NODE_PADDING;
        for badge in field.badges.iter().rev() {
            let label = badge.label();
            let badge_width = label.len() as f32 * BADGE_FONT_SIZE * CHARACTER_WIDTH + 6.0;
            let badge_x = right - badge_width;
            let badge_height = BADGE_FONT_SIZE + 4.0;
            let badge_color = badge_color(badge);
            writeln!(
                svg,
                r#"<rect x="{badge_x:.1}" y="{:.1}" width="{badge_width:.1}" height="{badge_height:.1}" rx="3"{}/><text x="{:.1}" y="{row_middle:.1}" dominant-baseline="central" text-anchor="middle" font-size="{BADGE_FONT_SIZE}" font-weight="600"{}>{label}</text>"#,
                row_middle - badge_height / 2.0,
                paint("fill", badge_color.opacity(0.2)),
                badge_x + badge_width / 2.0,
                paint("fill", badge_color)
            )
            .ok();
            right = badge_x - 4.0;
        }
        writeln!(
            svg,
            r#"<text x="{right:.1}" y="{row_middle:.1}" dominant-baseline="central" text-anchor="end" font-size="{:.2}"{}>{}</text>"#,
            FONT_SIZE * 0.85,
            paint("fill", palette.muted),
            escape(&field.type_label)
        )
        .ok();
        let marker = if field.comment.is_some() {
            format!(r#"<tspan dx="4"{}>•</tspan>"#, paint("fill", palette.muted))
        } else {
            String::new()
        };
        let label_room = right
            - text_x
            - field.type_label.chars().count() as f32 * FONT_SIZE * 0.85 * CHARACTER_WIDTH
            - if field.comment.is_some() {
                COMMENT_MARKER_WIDTH
            } else {
                0.0
            };
        writeln!(
            svg,
            r#"<text x="{text_x:.1}" y="{row_middle:.1}" dominant-baseline="central"{}>{}{marker}</text>"#,
            paint("fill", palette.foreground),
            escape(&truncate(&field.label, label_room, FONT_SIZE))
        )
        .ok();
        svg.push_str("</g>\n");
    }

    writeln!(
        svg,
        r#"<rect x="{x:.1}" y="{y:.1}" width="{width:.1}" height="{height:.1}" rx="{radius}" fill="none"{}{}/>"#,
        paint("stroke", palette.border),
        if node.referenced_only {
            r#" stroke-dasharray="4 3""#
        } else {
            ""
        }
    )
    .ok();
    svg.push_str("</g>\n");
}

/// The poly-line as path data, each bend rounded as the canvas rounds it.
fn rounded_path(points: &[Point<f32>], radius: f32) -> String {
    let mut path = String::new();
    let Some(first) = points.first() else {
        return path;
    };
    write!(path, "M{:.1},{:.1}", first.x, first.y).ok();
    let distance =
        |a: Point<f32>, b: Point<f32>| ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt();
    let lerp = |from: Point<f32>, to: Point<f32>, t: f32| {
        Point::new(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t)
    };
    for window in points.windows(3) {
        let [before, corner, after] = [window[0], window[1], window[2]];
        let to_before = distance(corner, before);
        let to_after = distance(corner, after);
        let bend = radius.min(to_before / 2.0).min(to_after / 2.0);
        if bend < 0.5 {
            write!(path, " L{:.1},{:.1}", corner.x, corner.y).ok();
            continue;
        }
        let start = lerp(corner, before, bend / to_before);
        let end = lerp(corner, after, bend / to_after);
        write!(
            path,
            " L{:.1},{:.1} Q{:.1},{:.1} {:.1},{:.1}",
            start.x, start.y, corner.x, corner.y, end.x, end.y
        )
        .ok();
    }
    if let Some(last) = points.last().filter(|_| points.len() > 1) {
        write!(path, " L{:.1},{:.1}", last.x, last.y).ok();
    }
    path
}

/// The text cut to about `width` with an ellipsis, as the screen truncates
/// it, since SVG text does not wrap or clip by itself.
fn truncate(text: &str, width: f32, font_size: f32) -> String {
    let fits = (width / (font_size * CHARACTER_WIDTH)).floor().max(1.0) as usize;
    if text.chars().count() <= fits {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(fits.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// A `fill` or `stroke` attribute in `#rrggbb`, with an opacity attribute
/// for a translucent color, since SVG 1.1 viewers do not read `rgba()`.
fn paint(attribute: &str, color: Hsla) -> String {
    if color.a >= 0.999 {
        format!(r#" {attribute}="{}""#, hex(color))
    } else {
        format!(
            r#" {attribute}="{}" {attribute}-opacity="{:.2}""#,
            hex(color),
            color.a
        )
    }
}

fn hex(color: Hsla) -> String {
    let rgba = color.to_rgb();
    let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        channel(rgba.r),
        channel(rgba.g),
        channel(rgba.b)
    )
}

fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use gpui::{Size, hsla};

    use super::super::{FieldBadge, GraphEdge, GraphNodeField, compute_node_sizes, layout_dag};
    use super::*;

    fn palette() -> Palette {
        let gray = hsla(0.0, 0.0, 0.5, 1.0);
        Palette {
            canvas: gray,
            node: gray,
            header: gray,
            border: gray,
            foreground: gray,
            muted: gray,
            edge: gray.opacity(0.5),
        }
    }

    fn table(id: &str, comment: Option<&str>, columns: &[(&str, Option<&str>)]) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            title: id.to_string().into(),
            comment: comment.map(|comment| comment.to_string().into()),
            schema: None,
            referenced_only: false,
            fields: columns
                .iter()
                .map(|(name, comment)| GraphNodeField {
                    label: name.to_string().into(),
                    type_label: "integer".into(),
                    badges: if *name == "id" {
                        vec![FieldBadge::PrimaryKey]
                    } else {
                        Vec::new()
                    },
                    comment: comment.map(|comment| comment.to_string().into()),
                })
                .collect(),
            position: Point::default(),
            size: Size::default(),
        }
    }

    fn model() -> GraphModel {
        let mut nodes = vec![
            table("customers", Some("People who <buy> & pay"), &[("id", None)]),
            table(
                "orders",
                None,
                &[("id", None), ("customer_id", Some("Who \"ordered\""))],
            ),
            table("lines", None, &[("id", None), ("order_id", None)]),
        ];
        let edges = vec![
            GraphEdge {
                from_node: "orders".into(),
                from_field_index: 1,
                to_node: "customers".into(),
                to_field_index: 0,
                dashed: false,
            },
            GraphEdge {
                from_node: "lines".into(),
                from_field_index: 1,
                to_node: "orders".into(),
                to_field_index: 0,
                dashed: true,
            },
        ];
        compute_node_sizes(&mut nodes);
        layout_dag(&mut nodes, &edges);
        GraphModel { nodes, edges }
    }

    fn export(model: &GraphModel, legend: &[(String, Hsla)]) -> String {
        let palette = palette();
        write_svg(
            model,
            &palette,
            legend,
            &|_| palette.header,
            &|_| palette.muted,
            "Inter",
        )
    }

    #[test]
    fn the_export_holds_every_table_and_edge_with_its_size() {
        let model = model();
        let svg = export(&model, &[]);

        let right = model
            .nodes
            .iter()
            .map(|node| node.position.x + node.size.width)
            .fold(f32::MIN, f32::max);
        let left = model
            .nodes
            .iter()
            .map(|node| node.position.x)
            .fold(f32::MAX, f32::min);
        let width = format!("{:.0}", right - left + MARGIN * 2.0);
        assert!(
            svg.starts_with(&format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}""#
            )),
            "{svg}"
        );
        assert!(svg.contains(&format!(r#"viewBox="0 0 {width} "#)));
        for title in ["customers", "orders", "lines"] {
            assert!(svg.contains(&format!(">{title}</text>")), "{title} missing");
        }
        // No table is selected, which on screen draws no edges at all.
        let edges = svg
            .lines()
            .filter(|line| line.starts_with("<path") && line.contains(r#"fill="none""#))
            .count();
        assert_eq!(edges, 2);
        assert_eq!(svg.matches(r#"stroke-dasharray="6 4""#).count(), 1);
        assert!(svg.contains(">PK</text>"));
        assert!(svg.trim_end().ends_with("</svg>"));
    }

    #[test]
    fn comments_are_escaped_and_marked() {
        let svg = export(&model(), &[]);
        assert!(svg.contains("<title>People who &lt;buy&gt; &amp; pay</title>"));
        assert!(svg.contains("<title>Who &quot;ordered&quot;</title>"));
        assert_eq!(svg.matches("•</tspan>").count(), 1);
        assert!(!svg.contains("<buy>"));
    }

    #[test]
    fn a_legend_adds_its_rows_below_the_graph() {
        let model = model();
        let without = export(&model, &[]);
        let legend = vec![
            ("public".to_string(), hsla(0.6, 0.5, 0.5, 1.0)),
            ("sales".to_string(), hsla(0.3, 0.5, 0.5, 1.0)),
        ];
        let with = export(&model, &legend);
        let height = |svg: &str| -> f32 {
            svg.split(r#"height=""#)
                .nth(1)
                .and_then(|rest| rest.split('"').next())
                .and_then(|value| value.parse().ok())
                .unwrap_or_default()
        };
        assert_eq!(
            height(&with) - height(&without),
            (MARGIN / 2.0 + 2.0 * LEGEND_ROW_HEIGHT).round()
        );
        assert!(with.contains(">sales</text>"));
    }

    #[test]
    fn long_text_is_cut_with_an_ellipsis() {
        assert_eq!(truncate("short", 200.0, 13.0), "short");
        let cut = truncate(&"x".repeat(100), 78.0, 13.0);
        assert_eq!(cut.chars().count(), 10);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn a_comment_makes_the_header_taller() {
        let mut nodes = vec![
            table("plain", None, &[("id", None)]),
            table("commented", Some("Explains it"), &[("id", None)]),
        ];
        compute_node_sizes(&mut nodes);
        assert_eq!(
            nodes[1].size.height - nodes[0].size.height,
            NODE_SUBTITLE_HEIGHT
        );
        assert_eq!(
            field_y_offset(&nodes[1], 0) - field_y_offset(&nodes[0], 0),
            NODE_SUBTITLE_HEIGHT
        );
    }
}
