//! Geometry cell values.
//!
//! PostGIS sends `geometry` and `geography` values as EWKB, which reads as a
//! wall of hex. The driver decodes it into a [`Geometry`] and the cell carries
//! its EWKT (`SRID=4326;POINT(18.07 59.33)`), which PostGIS takes back as
//! input, so a value that is written back unchanged keeps its SRID. Only the
//! simple feature types are decoded; curves and surfaces stay hex, which
//! PostGIS also accepts.

use serde_json::{Map, Number, Value};

const SRID_PREFIX: &str = "SRID=";

/// Which coordinates each position of a geometry has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dimensions {
    Xy,
    Xyz,
    Xym,
    Xyzm,
}

impl Dimensions {
    fn new(has_z: bool, has_m: bool) -> Self {
        match (has_z, has_m) {
            (false, false) => Self::Xy,
            (true, false) => Self::Xyz,
            (false, true) => Self::Xym,
            (true, true) => Self::Xyzm,
        }
    }

    /// Dimensions of a position with `count` coordinates and no tag saying
    /// what the third one is: WKT reads a third coordinate as Z.
    fn from_count(count: usize) -> Option<Self> {
        match count {
            2 => Some(Self::Xy),
            3 => Some(Self::Xyz),
            4 => Some(Self::Xyzm),
            _ => None,
        }
    }

    fn count(self) -> usize {
        match self {
            Self::Xy => 2,
            Self::Xyz | Self::Xym => 3,
            Self::Xyzm => 4,
        }
    }

    fn has_z(self) -> bool {
        matches!(self, Self::Xyz | Self::Xyzm)
    }

    fn tag(self) -> &'static str {
        match self {
            Self::Xy => "",
            Self::Xyz => "Z",
            Self::Xym => "M",
            Self::Xyzm => "ZM",
        }
    }
}

type Position = Vec<f64>;
type Ring = Vec<Position>;

#[derive(Debug, Clone, PartialEq)]
enum Shape {
    /// `None` is `POINT EMPTY`.
    Point(Option<Position>),
    LineString(Vec<Position>),
    Polygon(Vec<Ring>),
    MultiPoint(Vec<Option<Position>>),
    MultiLineString(Vec<Vec<Position>>),
    MultiPolygon(Vec<Vec<Ring>>),
    GeometryCollection(Vec<Shape>),
}

impl Shape {
    fn name(&self) -> &'static str {
        match self {
            Self::Point(_) => "POINT",
            Self::LineString(_) => "LINESTRING",
            Self::Polygon(_) => "POLYGON",
            Self::MultiPoint(_) => "MULTIPOINT",
            Self::MultiLineString(_) => "MULTILINESTRING",
            Self::MultiPolygon(_) => "MULTIPOLYGON",
            Self::GeometryCollection(_) => "GEOMETRYCOLLECTION",
        }
    }

    fn is_empty(&self) -> bool {
        match self {
            Self::Point(position) => position.is_none(),
            Self::LineString(items) => items.is_empty(),
            Self::Polygon(items) => items.is_empty(),
            Self::MultiPoint(items) => items.is_empty(),
            Self::MultiLineString(items) => items.is_empty(),
            Self::MultiPolygon(items) => items.is_empty(),
            Self::GeometryCollection(items) => items.is_empty(),
        }
    }
}

/// A decoded geometry: its spatial reference, dimensions and shape.
#[derive(Debug, Clone, PartialEq)]
pub struct Geometry {
    srid: Option<u32>,
    dimensions: Dimensions,
    shape: Shape,
}

impl Geometry {
    /// The spatial reference id, when the value carries one other than 0.
    pub fn srid(&self) -> Option<u32> {
        self.srid
    }

    pub fn dimensions(&self) -> Dimensions {
        self.dimensions
    }

    /// The geometry as WKT, as `ST_AsText` writes it.
    pub fn to_wkt(&self) -> String {
        let mut text = String::new();
        write_shape(&mut text, &self.shape, self.dimensions);
        text
    }

    /// The geometry as EWKT: [`Self::to_wkt`] behind its SRID.
    pub fn to_ewkt(&self) -> String {
        match self.srid {
            Some(srid) => format!("{SRID_PREFIX}{srid};{}", self.to_wkt()),
            None => self.to_wkt(),
        }
    }

    /// The geometry as a GeoJSON geometry object. GeoJSON has no M and no
    /// reference system, so M values and the SRID are left out.
    pub fn to_geojson(&self) -> Value {
        shape_geojson(&self.shape, self.dimensions)
    }

    /// A point's coordinates, `x, y` and then z, for a grid cell.
    fn point_coordinates(&self) -> Option<String> {
        let Shape::Point(Some(position)) = &self.shape else {
            return None;
        };
        let shown = if self.dimensions.has_z() { 3 } else { 2 };
        Some(
            position
                .iter()
                .take(shown)
                .map(|coordinate| format_number(*coordinate))
                .collect::<Vec<_>>()
                .join(", "),
        )
    }
}

/// The cell text for an EWKB value, or `None` when it is not a simple
/// feature the decoder knows.
pub fn ewkb_to_ewkt(bytes: &[u8]) -> Option<String> {
    from_ewkb(bytes).map(|geometry| geometry.to_ewkt())
}

/// Decode EWKB (or ISO WKB). The whole input must be one geometry.
pub fn from_ewkb(bytes: &[u8]) -> Option<Geometry> {
    let mut reader = WkbReader { bytes, offset: 0 };
    let (srid, dimensions, shape) = reader.geometry()?;
    (reader.offset == bytes.len()).then_some(Geometry {
        srid: srid.filter(|srid| *srid != 0),
        dimensions,
        shape,
    })
}

/// Parse WKT, or EWKT with a `SRID=n;` prefix.
pub fn parse_ewkt(text: &str) -> Result<Geometry, String> {
    let (srid, wkt) = split_srid(text)?;
    let tokens = tokenize(wkt)?;
    let mut parser = WktParser {
        tokens,
        index: 0,
        coordinate_count: None,
        declared: None,
        end: wkt.chars().count(),
    };
    let shape = parser.geometry()?;
    if let Some((_, position)) = parser.tokens.get(parser.index) {
        return Err(format!(
            "unexpected text after the geometry at character {}",
            position + 1
        ));
    }
    let dimensions = match (parser.declared, parser.coordinate_count) {
        (Some(declared), _) => declared,
        (None, Some(count)) => Dimensions::from_count(count)
            .ok_or_else(|| format!("a position has {count} coordinates"))?,
        (None, None) => Dimensions::Xy,
    };
    Ok(Geometry {
        srid: srid.filter(|srid| *srid != 0),
        dimensions,
        shape,
    })
}

/// Why `text` will not be read as a geometry, for an edit to a geometry
/// column. `None` for WKT and EWKT that parse, for hex EWKB, and for curve
/// and surface types, which PostGIS reads but this parser does not.
pub fn edit_error(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if !trimmed.is_empty() && trimmed.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    if let Ok((_, wkt)) = split_srid(trimmed) {
        let name: String = wkt
            .trim_start()
            .chars()
            .take_while(char::is_ascii_alphabetic)
            .collect::<String>()
            .to_ascii_uppercase();
        if UNSUPPORTED_TYPES
            .iter()
            .any(|unsupported| name.starts_with(unsupported))
        {
            return None;
        }
    }
    parse_ewkt(trimmed).err()
}

/// `edited` behind the SRID of `original`, when `edited` is WKT without one
/// and `original` had one. A `geometry` column without an SRID in its type
/// stores WKT as SRID 0, so WKT typed over a value would otherwise drop that
/// value's reference system.
pub fn carry_srid(edited: &str, original: &str) -> Option<String> {
    let (edited_srid, _) = split_srid(edited).ok()?;
    if edited_srid.is_some() || parse_ewkt(edited).is_err() {
        return None;
    }
    let (Some(srid), _) = split_srid(original).ok()? else {
        return None;
    };
    Some(format!("{SRID_PREFIX}{srid};{}", edited.trim()))
}

/// How a stored geometry reads in the grid: WKT without the SRID, and a
/// point as its coordinates. `None` when the text is not EWKT.
pub fn display_text(stored: &str) -> Option<String> {
    let (srid, wkt) = split_srid(stored).ok()?;
    let wkt = wkt.trim();
    let starts_with_point = wkt
        .get(..5)
        .is_some_and(|start| start.eq_ignore_ascii_case("POINT"));
    if starts_with_point
        && let Ok(geometry) = parse_ewkt(wkt)
        && let Some(coordinates) = geometry.point_coordinates()
    {
        return Some(coordinates);
    }
    srid.map(|_| wkt.to_string())
}

/// The text after an optional `SRID=n;` prefix, and n.
fn split_srid(text: &str) -> Result<(Option<u32>, &str), String> {
    let text = text.trim();
    let has_prefix = text
        .get(..SRID_PREFIX.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(SRID_PREFIX));
    if !has_prefix {
        return Ok((None, text));
    }
    let rest = &text[SRID_PREFIX.len()..];
    let (srid, wkt) = rest
        .split_once(';')
        .ok_or_else(|| "expected ';' after the SRID".to_string())?;
    let srid = srid
        .trim()
        .parse::<u32>()
        .map_err(|_| format!("the SRID '{}' is not a number", srid.trim()))?;
    Ok((Some(srid), wkt))
}

/// Shortest text that reads back as the same number, and 0 for -0 as
/// `ST_AsText` writes it.
fn format_number(value: f64) -> String {
    if value == 0.0 {
        return "0".to_string();
    }
    value.to_string()
}

fn write_position(text: &mut String, position: &[f64]) {
    for (index, coordinate) in position.iter().enumerate() {
        if index > 0 {
            text.push(' ');
        }
        text.push_str(&format_number(*coordinate));
    }
}

fn write_positions(text: &mut String, positions: &[Position]) {
    text.push('(');
    for (index, position) in positions.iter().enumerate() {
        if index > 0 {
            text.push(',');
        }
        write_position(text, position);
    }
    text.push(')');
}

fn write_rings(text: &mut String, rings: &[Ring]) {
    text.push('(');
    for (index, ring) in rings.iter().enumerate() {
        if index > 0 {
            text.push(',');
        }
        write_positions(text, ring);
    }
    text.push(')');
}

fn write_shape(text: &mut String, shape: &Shape, dimensions: Dimensions) {
    text.push_str(shape.name());
    let tag = dimensions.tag();
    if !tag.is_empty() {
        text.push(' ');
        text.push_str(tag);
        text.push(' ');
    }
    if shape.is_empty() {
        if tag.is_empty() {
            text.push(' ');
        }
        text.push_str("EMPTY");
        return;
    }
    match shape {
        Shape::Point(Some(position)) => {
            text.push('(');
            write_position(text, position);
            text.push(')');
        }
        Shape::Point(None) => {}
        Shape::LineString(positions) => write_positions(text, positions),
        Shape::Polygon(rings) => write_rings(text, rings),
        Shape::MultiPoint(points) => {
            text.push('(');
            for (index, point) in points.iter().enumerate() {
                if index > 0 {
                    text.push(',');
                }
                match point {
                    Some(position) => {
                        text.push('(');
                        write_position(text, position);
                        text.push(')');
                    }
                    None => text.push_str("EMPTY"),
                }
            }
            text.push(')');
        }
        Shape::MultiLineString(lines) => write_rings(text, lines),
        Shape::MultiPolygon(polygons) => {
            text.push('(');
            for (index, polygon) in polygons.iter().enumerate() {
                if index > 0 {
                    text.push(',');
                }
                write_rings(text, polygon);
            }
            text.push(')');
        }
        Shape::GeometryCollection(members) => {
            text.push('(');
            for (index, member) in members.iter().enumerate() {
                if index > 0 {
                    text.push(',');
                }
                write_shape(text, member, dimensions);
            }
            text.push(')');
        }
    }
}

fn position_geojson(position: &[f64], dimensions: Dimensions) -> Value {
    let shown = if dimensions.has_z() { 3 } else { 2 };
    Value::Array(
        position
            .iter()
            .take(shown)
            .map(|coordinate| Number::from_f64(*coordinate).map_or(Value::Null, Value::Number))
            .collect(),
    )
}

fn positions_geojson(positions: &[Position], dimensions: Dimensions) -> Value {
    Value::Array(
        positions
            .iter()
            .map(|position| position_geojson(position, dimensions))
            .collect(),
    )
}

fn rings_geojson(rings: &[Ring], dimensions: Dimensions) -> Value {
    Value::Array(
        rings
            .iter()
            .map(|ring| positions_geojson(ring, dimensions))
            .collect(),
    )
}

fn shape_geojson(shape: &Shape, dimensions: Dimensions) -> Value {
    let (geojson_type, member, value) = match shape {
        Shape::Point(position) => (
            "Point",
            "coordinates",
            position
                .as_deref()
                .map_or(Value::Array(Vec::new()), |position| {
                    position_geojson(position, dimensions)
                }),
        ),
        Shape::LineString(positions) => (
            "LineString",
            "coordinates",
            positions_geojson(positions, dimensions),
        ),
        Shape::Polygon(rings) => ("Polygon", "coordinates", rings_geojson(rings, dimensions)),
        Shape::MultiPoint(points) => (
            "MultiPoint",
            "coordinates",
            // GeoJSON cannot hold an empty member point.
            Value::Array(
                points
                    .iter()
                    .flatten()
                    .map(|position| position_geojson(position, dimensions))
                    .collect(),
            ),
        ),
        Shape::MultiLineString(lines) => (
            "MultiLineString",
            "coordinates",
            rings_geojson(lines, dimensions),
        ),
        Shape::MultiPolygon(polygons) => (
            "MultiPolygon",
            "coordinates",
            Value::Array(
                polygons
                    .iter()
                    .map(|polygon| rings_geojson(polygon, dimensions))
                    .collect(),
            ),
        ),
        Shape::GeometryCollection(members) => (
            "GeometryCollection",
            "geometries",
            Value::Array(
                members
                    .iter()
                    .map(|member| shape_geojson(member, dimensions))
                    .collect(),
            ),
        ),
    };
    let mut object = Map::new();
    object.insert("type".to_string(), Value::String(geojson_type.to_string()));
    object.insert(member.to_string(), value);
    Value::Object(object)
}

struct WkbReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl WkbReader<'_> {
    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let end = self.offset.checked_add(N)?;
        let bytes = self.bytes.get(self.offset..end)?.try_into().ok()?;
        self.offset = end;
        Some(bytes)
    }

    fn u32(&mut self, little_endian: bool) -> Option<u32> {
        let bytes = self.take::<4>()?;
        Some(if little_endian {
            u32::from_le_bytes(bytes)
        } else {
            u32::from_be_bytes(bytes)
        })
    }

    fn f64(&mut self, little_endian: bool) -> Option<f64> {
        let bytes = self.take::<8>()?;
        Some(if little_endian {
            f64::from_le_bytes(bytes)
        } else {
            f64::from_be_bytes(bytes)
        })
    }

    /// A count of items that each take at least `item_size` bytes, refused
    /// when the input is too short to hold them, so a corrupt count cannot
    /// make the reader allocate gigabytes.
    fn count(&mut self, little_endian: bool, item_size: usize) -> Option<usize> {
        let count = usize::try_from(self.u32(little_endian)?).ok()?;
        let remaining = self.bytes.len().saturating_sub(self.offset);
        (count.checked_mul(item_size)? <= remaining).then_some(count)
    }

    fn position(&mut self, little_endian: bool, dimensions: Dimensions) -> Option<Position> {
        (0..dimensions.count())
            .map(|_| self.f64(little_endian))
            .collect()
    }

    fn positions(&mut self, little_endian: bool, dimensions: Dimensions) -> Option<Vec<Position>> {
        let count = self.count(little_endian, dimensions.count() * 8)?;
        (0..count)
            .map(|_| self.position(little_endian, dimensions))
            .collect()
    }

    fn rings(&mut self, little_endian: bool, dimensions: Dimensions) -> Option<Vec<Ring>> {
        let count = self.count(little_endian, 4)?;
        (0..count)
            .map(|_| self.positions(little_endian, dimensions))
            .collect()
    }

    fn members(&mut self, little_endian: bool) -> Option<Vec<Shape>> {
        // A member is at least a byte order and a type.
        let count = self.count(little_endian, 5)?;
        (0..count)
            .map(|_| self.geometry().map(|(_, _, shape)| shape))
            .collect()
    }

    fn geometry(&mut self) -> Option<(Option<u32>, Dimensions, Shape)> {
        let little_endian = match self.take::<1>()? {
            [0] => false,
            [1] => true,
            _ => return None,
        };
        let raw_type = self.u32(little_endian)?;
        let code = raw_type & 0x0fff_ffff;
        let (iso_z, iso_m) = match code / 1000 {
            0 => (false, false),
            1 => (true, false),
            2 => (false, true),
            3 => (true, true),
            _ => return None,
        };
        let dimensions = Dimensions::new(
            raw_type & 0x8000_0000 != 0 || iso_z,
            raw_type & 0x4000_0000 != 0 || iso_m,
        );
        let srid = if raw_type & 0x2000_0000 != 0 {
            Some(self.u32(little_endian)?)
        } else {
            None
        };
        let shape = match code % 1000 {
            1 => {
                let position = self.position(little_endian, dimensions)?;
                // An empty point is written with every coordinate NaN.
                Shape::Point((!position.iter().all(|value| value.is_nan())).then_some(position))
            }
            2 => Shape::LineString(self.positions(little_endian, dimensions)?),
            3 => Shape::Polygon(self.rings(little_endian, dimensions)?),
            4 => Shape::MultiPoint(
                self.members(little_endian)?
                    .into_iter()
                    .map(|member| match member {
                        Shape::Point(position) => Some(position),
                        _ => None,
                    })
                    .collect::<Option<_>>()?,
            ),
            5 => Shape::MultiLineString(
                self.members(little_endian)?
                    .into_iter()
                    .map(|member| match member {
                        Shape::LineString(positions) => Some(positions),
                        _ => None,
                    })
                    .collect::<Option<_>>()?,
            ),
            6 => Shape::MultiPolygon(
                self.members(little_endian)?
                    .into_iter()
                    .map(|member| match member {
                        Shape::Polygon(rings) => Some(rings),
                        _ => None,
                    })
                    .collect::<Option<_>>()?,
            ),
            7 => Shape::GeometryCollection(self.members(little_endian)?),
            _ => return None,
        };
        Some((srid, dimensions, shape))
    }
}

/// Types PostGIS reads as WKT that [`parse_ewkt`] does not.
const UNSUPPORTED_TYPES: &[&str] = &[
    "CIRCULARSTRING",
    "COMPOUNDCURVE",
    "CURVEPOLYGON",
    "MULTICURVE",
    "MULTISURFACE",
    "POLYHEDRALSURFACE",
    "TIN",
    "TRIANGLE",
];

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Word(String),
    Number(f64),
    Open,
    Close,
    Comma,
}

/// Tokens with the character each starts at.
fn tokenize(text: &str) -> Result<Vec<(Token, usize)>, String> {
    let mut tokens = Vec::new();
    let mut chars = text.chars().enumerate().peekable();
    while let Some(&(position, character)) = chars.peek() {
        match character {
            _ if character.is_whitespace() => {
                chars.next();
            }
            '(' | ')' | ',' => {
                chars.next();
                let token = match character {
                    '(' => Token::Open,
                    ')' => Token::Close,
                    _ => Token::Comma,
                };
                tokens.push((token, position));
            }
            _ if character.is_ascii_alphabetic() => {
                let mut word = String::new();
                while let Some(&(_, character)) = chars.peek()
                    && character.is_ascii_alphabetic()
                {
                    word.push(character.to_ascii_uppercase());
                    chars.next();
                }
                tokens.push((Token::Word(word), position));
            }
            _ if character.is_ascii_digit() || matches!(character, '-' | '+' | '.') => {
                let mut number = String::new();
                while let Some(&(_, character)) = chars.peek()
                    && (character.is_ascii_digit()
                        || matches!(character, '-' | '+' | '.' | 'e' | 'E'))
                {
                    number.push(character);
                    chars.next();
                }
                let value = number.parse::<f64>().map_err(|_| {
                    format!("'{number}' at character {} is not a number", position + 1)
                })?;
                tokens.push((Token::Number(value), position));
            }
            _ => {
                return Err(format!(
                    "unexpected '{character}' at character {}",
                    position + 1
                ));
            }
        }
    }
    Ok(tokens)
}

#[derive(Clone, Copy, PartialEq)]
enum ShapeName {
    Point,
    LineString,
    Polygon,
    MultiPoint,
    MultiLineString,
    MultiPolygon,
    GeometryCollection,
}

const SHAPE_NAMES: &[(&str, ShapeName)] = &[
    ("POINT", ShapeName::Point),
    ("LINESTRING", ShapeName::LineString),
    ("POLYGON", ShapeName::Polygon),
    ("MULTIPOINT", ShapeName::MultiPoint),
    ("MULTILINESTRING", ShapeName::MultiLineString),
    ("MULTIPOLYGON", ShapeName::MultiPolygon),
    ("GEOMETRYCOLLECTION", ShapeName::GeometryCollection),
];

fn tag_dimensions(tag: &str) -> Option<Dimensions> {
    match tag {
        "Z" => Some(Dimensions::Xyz),
        "M" => Some(Dimensions::Xym),
        "ZM" => Some(Dimensions::Xyzm),
        _ => None,
    }
}

struct WktParser {
    tokens: Vec<(Token, usize)>,
    index: usize,
    /// How many coordinates the positions read so far have; they must agree.
    coordinate_count: Option<usize>,
    /// The first dimension tag, which says what a third coordinate is.
    declared: Option<Dimensions>,
    /// Character count of the input, for errors at its end.
    end: usize,
}

impl WktParser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.index).map(|(token, _)| token)
    }

    fn position(&self) -> usize {
        self.tokens
            .get(self.index)
            .map_or(self.end, |(_, position)| *position)
            + 1
    }

    fn expected(&self, what: &str) -> String {
        match self.tokens.get(self.index) {
            Some((_, position)) => format!("expected {what} at character {}", position + 1),
            None => format!("expected {what} at the end"),
        }
    }

    fn expect(&mut self, token: Token, what: &str) -> Result<(), String> {
        if self.peek() == Some(&token) {
            self.index += 1;
            Ok(())
        } else {
            Err(self.expected(what))
        }
    }

    /// Consume `EMPTY` when it is next.
    fn empty(&mut self) -> bool {
        if matches!(self.peek(), Some(Token::Word(word)) if word == "EMPTY") {
            self.index += 1;
            true
        } else {
            false
        }
    }

    /// A type name with its optional dimension tag, either attached
    /// (`POINTM`) or a word of its own (`POINT ZM`).
    fn type_name(&mut self) -> Result<ShapeName, String> {
        let Some(Token::Word(word)) = self.peek().cloned() else {
            return Err(self.expected("a geometry type"));
        };
        let position = self.position();
        let (shape, attached) = SHAPE_NAMES
            .iter()
            .find_map(|(name, shape)| {
                let rest = word.strip_prefix(name)?;
                if rest.is_empty() {
                    Some((*shape, None))
                } else {
                    tag_dimensions(rest).map(|dimensions| (*shape, Some(dimensions)))
                }
            })
            .ok_or_else(|| format!("unknown geometry type '{word}' at character {position}"))?;
        self.index += 1;
        let tag = match (attached, self.peek()) {
            (None, Some(Token::Word(word))) => match tag_dimensions(word) {
                Some(dimensions) => {
                    self.index += 1;
                    Some(dimensions)
                }
                None => None,
            },
            _ => attached,
        };
        if let Some(tag) = tag {
            if self.declared.is_some_and(|declared| declared != tag) {
                return Err(format!(
                    "{} does not match the dimensions before it at character {position}",
                    tag.tag()
                ));
            }
            self.check_count(tag.count())?;
            self.declared = Some(tag);
        }
        Ok(shape)
    }

    fn check_count(&mut self, count: usize) -> Result<(), String> {
        match self.coordinate_count {
            Some(expected) if expected != count => Err(format!(
                "expected {expected} coordinates per position, found {count} at character {}",
                self.position()
            )),
            _ => {
                self.coordinate_count = Some(count);
                Ok(())
            }
        }
    }

    fn coordinates(&mut self) -> Result<Position, String> {
        let mut position = Vec::new();
        while let Some(Token::Number(value)) = self.peek() {
            position.push(*value);
            self.index += 1;
        }
        if position.len() < 2 {
            return Err(self.expected("a coordinate"));
        }
        if position.len() > 4 {
            return Err(format!(
                "a position has {} coordinates at character {}",
                position.len(),
                self.position()
            ));
        }
        self.check_count(position.len())?;
        Ok(position)
    }

    /// `(` items separated by commas `)`.
    fn list<T>(
        &mut self,
        mut item: impl FnMut(&mut Self) -> Result<T, String>,
    ) -> Result<Vec<T>, String> {
        self.expect(Token::Open, "'('")?;
        let mut items = vec![item(self)?];
        while self.peek() == Some(&Token::Comma) {
            self.index += 1;
            items.push(item(self)?);
        }
        self.expect(Token::Close, "',' or ')'")?;
        Ok(items)
    }

    fn positions(&mut self) -> Result<Vec<Position>, String> {
        if self.empty() {
            return Ok(Vec::new());
        }
        self.list(Self::coordinates)
    }

    fn rings(&mut self) -> Result<Vec<Ring>, String> {
        if self.empty() {
            return Ok(Vec::new());
        }
        self.list(Self::positions)
    }

    /// A member of a multipoint: `(1 2)`, `1 2` or `EMPTY`.
    fn member_point(&mut self) -> Result<Option<Position>, String> {
        if self.empty() {
            return Ok(None);
        }
        if self.peek() == Some(&Token::Open) {
            self.index += 1;
            let position = self.coordinates()?;
            self.expect(Token::Close, "')'")?;
            Ok(Some(position))
        } else {
            self.coordinates().map(Some)
        }
    }

    fn geometry(&mut self) -> Result<Shape, String> {
        let name = self.type_name()?;
        if self.empty() {
            let shape = match name {
                ShapeName::Point => Shape::Point(None),
                ShapeName::LineString => Shape::LineString(Vec::new()),
                ShapeName::Polygon => Shape::Polygon(Vec::new()),
                ShapeName::MultiPoint => Shape::MultiPoint(Vec::new()),
                ShapeName::MultiLineString => Shape::MultiLineString(Vec::new()),
                ShapeName::MultiPolygon => Shape::MultiPolygon(Vec::new()),
                ShapeName::GeometryCollection => Shape::GeometryCollection(Vec::new()),
            };
            return Ok(shape);
        }
        let shape = match name {
            ShapeName::Point => {
                self.expect(Token::Open, "'('")?;
                let position = self.coordinates()?;
                self.expect(Token::Close, "')'")?;
                Shape::Point(Some(position))
            }
            ShapeName::LineString => Shape::LineString(self.list(Self::coordinates)?),
            ShapeName::Polygon => Shape::Polygon(self.list(Self::positions)?),
            ShapeName::MultiPoint => Shape::MultiPoint(self.list(Self::member_point)?),
            ShapeName::MultiLineString => Shape::MultiLineString(self.list(Self::positions)?),
            ShapeName::MultiPolygon => Shape::MultiPolygon(self.list(Self::rings)?),
            ShapeName::GeometryCollection => Shape::GeometryCollection(self.list(Self::geometry)?),
        };
        Ok(shape)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> Vec<u8> {
        crate::binary::decode_lenient(text).expect("hex")
    }

    #[test]
    fn decodes_postgis_ewkb() {
        // SELECT 'SRID=4326;POINT(18.0686 59.3293)'::geometry
        let point = hex("0101000020E61000006F8104C58F11324052499D8026AA4D40");
        assert_eq!(
            ewkb_to_ewkt(&point).as_deref(),
            Some("SRID=4326;POINT(18.0686 59.3293)")
        );
        // SELECT 'LINESTRING(0 0,1 1,2 1)'::geometry
        let line = hex(
            "01020000000300000000000000000000000000000000000000000000000000F03F\
             000000000000F03F0000000000000040000000000000F03F",
        );
        assert_eq!(
            ewkb_to_ewkt(&line).as_deref(),
            Some("LINESTRING(0 0,1 1,2 1)")
        );
        // SELECT 'POINT Z (1 2 3)'::geometry
        let point_z = hex("0101000080000000000000F03F00000000000000400000000000000840");
        assert_eq!(ewkb_to_ewkt(&point_z).as_deref(), Some("POINT Z (1 2 3)"));
        // SELECT 'POINT EMPTY'::geometry
        let empty = hex("0101000000000000000000F87F000000000000F87F");
        assert_eq!(ewkb_to_ewkt(&empty).as_deref(), Some("POINT EMPTY"));
    }

    #[test]
    fn decodes_big_endian_and_iso_wkb() {
        let big_endian = hex("00000000013FF00000000000004000000000000000");
        assert_eq!(ewkb_to_ewkt(&big_endian).as_deref(), Some("POINT(1 2)"));
        // ISO type 2001 is POINT M.
        let iso_m = hex("01D1070000000000000000F03F00000000000000400000000000000840");
        assert_eq!(ewkb_to_ewkt(&iso_m).as_deref(), Some("POINT M (1 2 3)"));
    }

    #[test]
    fn refuses_what_is_not_one_geometry() {
        assert_eq!(from_ewkb(&[]), None);
        // Trailing bytes.
        assert_eq!(
            from_ewkb(&hex("0101000000000000000000F03F000000000000004000")),
            None
        );
        // Truncated.
        assert_eq!(from_ewkb(&hex("0101000000000000000000F03F")), None);
        // A linestring claiming four billion points.
        assert_eq!(from_ewkb(&hex("0102000000FFFFFFFF")), None);
        // CIRCULARSTRING is not decoded.
        assert_eq!(from_ewkb(&hex("010800000000000000")), None);
    }

    #[test]
    fn wkt_round_trips() {
        for text in [
            "POINT(1 2)",
            "SRID=4326;POINT(-73.9857 40.7484)",
            "POINT Z (1 2 3)",
            "POINT M (1 2 3)",
            "POINT ZM (1 2 3 4)",
            "POINT EMPTY",
            "LINESTRING(0 0,1 1)",
            "POLYGON((0 0,4 0,4 4,0 4,0 0),(1 1,2 1,2 2,1 1))",
            "MULTIPOINT((0 0),(1 1))",
            "MULTILINESTRING((0 0,1 1),(2 2,3 3))",
            "MULTIPOLYGON(((0 0,1 0,1 1,0 0)),((2 2,3 2,3 3,2 2)))",
            "GEOMETRYCOLLECTION(POINT(1 2),LINESTRING(0 0,1 1))",
            "GEOMETRYCOLLECTION EMPTY",
            "SRID=3857;MULTIPOLYGON Z (((0 0 1,1 0 1,1 1 1,0 0 1)))",
        ] {
            assert_eq!(parse_ewkt(text).map(|g| g.to_ewkt()).as_deref(), Ok(text));
        }
    }

    #[test]
    fn parses_the_spellings_postgis_takes() {
        let ewkt = |text: &str| parse_ewkt(text).map(|geometry| geometry.to_ewkt());
        assert_eq!(ewkt("point(1 2)").as_deref(), Ok("POINT(1 2)"));
        assert_eq!(ewkt("POINT (1.5e1 -2)").as_deref(), Ok("POINT(15 -2)"));
        assert_eq!(ewkt("POINTM(1 2 3)").as_deref(), Ok("POINT M (1 2 3)"));
        assert_eq!(ewkt("POINT(1 2 3)").as_deref(), Ok("POINT Z (1 2 3)"));
        assert_eq!(
            ewkt("MULTIPOINT(0 0, 1 1)").as_deref(),
            Ok("MULTIPOINT((0 0),(1 1))")
        );
        assert_eq!(
            ewkt("srid=4326; POINT(1 2)").as_deref(),
            Ok("SRID=4326;POINT(1 2)")
        );
        assert_eq!(ewkt("SRID=0;POINT(1 2)").as_deref(), Ok("POINT(1 2)"));
    }

    #[test]
    fn says_where_wkt_goes_wrong() {
        assert_eq!(
            parse_ewkt("POINT(1)"),
            Err("expected a coordinate at character 8".to_string())
        );
        assert_eq!(
            parse_ewkt("LINESTRING(0 0,1 1 1)"),
            Err("expected 2 coordinates per position, found 3 at character 21".to_string())
        );
        assert_eq!(
            parse_ewkt("POINT Z (1 2)"),
            Err("expected 3 coordinates per position, found 2 at character 13".to_string())
        );
        assert_eq!(
            parse_ewkt("CIRCLE(1 2)"),
            Err("unknown geometry type 'CIRCLE' at character 1".to_string())
        );
        assert_eq!(
            parse_ewkt("POINT(1 2"),
            Err("expected ')' at the end".to_string())
        );
        assert_eq!(
            parse_ewkt("POINT(1 2) x"),
            Err("unexpected text after the geometry at character 12".to_string())
        );
        assert_eq!(
            parse_ewkt("SRID=abc;POINT(1 2)"),
            Err("the SRID 'abc' is not a number".to_string())
        );
    }

    #[test]
    fn edits_accept_what_postgis_reads() {
        assert_eq!(edit_error("POINT(1 2)"), None);
        assert_eq!(edit_error("SRID=4326;POINT(1 2)"), None);
        assert_eq!(
            edit_error("0101000000000000000000F03F0000000000000040"),
            None
        );
        assert_eq!(edit_error("CIRCULARSTRING(0 0,1 1,1 0)"), None);
        assert_eq!(edit_error("SRID=4326;CURVEPOLYGON EMPTY"), None);
        assert!(edit_error("POINT(1 2").is_some());
        assert!(edit_error("").is_some());
    }

    #[test]
    fn typed_wkt_keeps_the_srid() {
        assert_eq!(
            carry_srid("POINT(1 2)", "SRID=4326;POINT(0 0)").as_deref(),
            Some("SRID=4326;POINT(1 2)")
        );
        assert_eq!(
            carry_srid("SRID=3006;POINT(1 2)", "SRID=4326;POINT(0 0)"),
            None
        );
        assert_eq!(carry_srid("POINT(1 2)", "POINT(0 0)"), None);
        assert_eq!(carry_srid("POINT(1", "SRID=4326;POINT(0 0)"), None);
    }

    #[test]
    fn grid_shows_points_as_coordinates() {
        assert_eq!(
            display_text("SRID=4326;POINT(18.0686 59.3293)").as_deref(),
            Some("18.0686, 59.3293")
        );
        assert_eq!(display_text("POINT Z (1 2 3)").as_deref(), Some("1, 2, 3"));
        assert_eq!(display_text("POINT M (1 2 3)").as_deref(), Some("1, 2"));
        assert_eq!(
            display_text("SRID=4326;LINESTRING(0 0,1 1)").as_deref(),
            Some("LINESTRING(0 0,1 1)")
        );
        assert_eq!(display_text("POINT EMPTY"), None);
        assert_eq!(display_text("LINESTRING(0 0,1 1)"), None);
    }

    #[test]
    fn geojson() {
        use serde_json::json;
        let geojson = |text: &str| parse_ewkt(text).map(|geometry| geometry.to_geojson());
        assert_eq!(
            geojson("SRID=4326;POINT(18.5 59.25)"),
            Ok(json!({"type": "Point", "coordinates": [18.5, 59.25]}))
        );
        assert_eq!(
            geojson("POINT ZM (1 2 3 4)"),
            Ok(json!({"type": "Point", "coordinates": [1.0, 2.0, 3.0]}))
        );
        assert_eq!(
            geojson("POLYGON((0 0,1 0,1 1,0 0))"),
            Ok(json!({
                "type": "Polygon",
                "coordinates": [[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 0.0]]]
            }))
        );
        assert_eq!(
            geojson("MULTIPOINT((1 2),EMPTY)"),
            Ok(json!({"type": "MultiPoint", "coordinates": [[1.0, 2.0]]}))
        );
        assert_eq!(
            geojson("GEOMETRYCOLLECTION(POINT(1 2))"),
            Ok(json!({
                "type": "GeometryCollection",
                "geometries": [{"type": "Point", "coordinates": [1.0, 2.0]}]
            }))
        );
        assert_eq!(
            geojson("POINT EMPTY"),
            Ok(json!({"type": "Point", "coordinates": []}))
        );
    }
}
