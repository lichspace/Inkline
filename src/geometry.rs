use bytemuck::{Pod, Zeroable};
use freedraw::{
    get_stroke_outline_points, get_stroke_points, InputPoint, StrokeOptions, TaperOptions,
    TaperType,
};
use lyon::math::point as lyon_point;
use lyon::path::{FillRule, Path};
use lyon::tessellation::{
    BuffersBuilder, FillOptions, FillTessellator, FillVertex, FillVertexConstructor, VertexBuffers,
};

use crate::model::Stroke;

pub const BASE_WIDTH: f32 = 5.0;

const CAP_SEGMENTS: usize = 24;
const THINNING: f64 = 0.62;
const SMOOTHING: f64 = 0.55;
const STREAMLINE: f64 = 0.55;
const TAPER_FRACTION: f64 = 0.18;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct GpuVertex {
    pub position: [f32; 2],
    pub color: [f32; 4],
}

#[derive(Clone, Debug, Default)]
pub struct GpuGeometry {
    pub vertices: Vec<GpuVertex>,
    pub indices: Vec<u32>,
}

impl GpuGeometry {
    pub fn append(&mut self, mut other: GpuGeometry) {
        let vertex_offset = self.vertices.len() as u32;
        self.vertices.append(&mut other.vertices);
        self.indices
            .extend(other.indices.into_iter().map(|index| index + vertex_offset));
    }
}

pub fn width_for_pressure(stroke: &Stroke, pressure: f32) -> f32 {
    let pressure = pressure.clamp(0.0, 1.0);
    BASE_WIDTH * stroke.width_scale * (0.25 + 0.75 * pressure)
}

pub fn tessellate_stroke(stroke: &Stroke) -> GpuGeometry {
    tessellate_stroke_with_options(stroke, true)
}

pub fn tessellate_stroke_with_options(stroke: &Stroke, complete: bool) -> GpuGeometry {
    let mut geometry = GpuGeometry::default();
    if stroke.points.is_empty() {
        return geometry;
    }

    let outline = stroke_outline(stroke, complete);
    if outline.len() < 3 {
        let point = &stroke.points[0];
        let center = lyon_point(point.x, point.y);
        let radius = (width_for_pressure(stroke, point.pressure) / 2.0).max(0.75);
        append_circle(&mut geometry, center, radius as f64, stroke.color);
        return geometry;
    }

    append_polygon(&mut geometry, &outline, stroke.color);
    geometry
}

pub fn stroke_outline(stroke: &Stroke, complete: bool) -> Vec<[f64; 2]> {
    if stroke.points.is_empty() {
        return Vec::new();
    }

    let size = (BASE_WIDTH * stroke.width_scale.max(0.05)) as f64;
    let input = stroke
        .points
        .iter()
        .map(|point| {
            InputPoint::Array(
                [point.x as f64, point.y as f64],
                Some(point.pressure as f64),
            )
        })
        .collect::<Vec<_>>();

    let mut options = base_options(size, complete);
    options.simulate_pressure = Some(!stroke_uses_real_pressure(stroke));

    let stroke_points = get_stroke_points(&input, &options);
    if stroke_points.is_empty() {
        return Vec::new();
    }

    let total_length = stroke_points
        .last()
        .map(|point| point.running_length)
        .unwrap_or(0.0);
    let taper_length = (total_length * TAPER_FRACTION).max(size * 0.75);
    let taper = TaperOptions {
        cap: Some(true),
        taper: Some(TaperType::Number(taper_length)),
        easing: None,
    };
    options.start = Some(taper.clone());
    options.end = Some(taper);

    get_stroke_outline_points(&stroke_points, &options)
}

pub fn stroke_svg_path(stroke: &Stroke) -> String {
    let outline = stroke_outline(stroke, true);
    outline_to_svg_path(&outline)
}

fn stroke_uses_real_pressure(stroke: &Stroke) -> bool {
    if stroke.points.len() < 2 {
        return false;
    }

    let mut min_pressure = f32::INFINITY;
    let mut max_pressure = f32::NEG_INFINITY;
    for point in &stroke.points {
        min_pressure = min_pressure.min(point.pressure);
        max_pressure = max_pressure.max(point.pressure);
    }
    max_pressure - min_pressure > 0.05
}

fn base_options(size: f64, complete: bool) -> StrokeOptions {
    StrokeOptions {
        size: Some(size),
        thinning: Some(THINNING),
        smoothing: Some(SMOOTHING),
        streamline: Some(STREAMLINE),
        easing: None,
        simulate_pressure: Some(true),
        start: Some(TaperOptions::default()),
        end: Some(TaperOptions::default()),
        last: Some(complete),
        closed: Some(false),
    }
}

fn outline_to_svg_path(outline: &[[f64; 2]]) -> String {
    if outline.len() < 3 {
        return String::new();
    }

    let first = outline[0];
    let mut path = format!("M {:.3} {:.3}", first[0], first[1]);
    for point in &outline[1..] {
        path.push_str(&format!(" L {:.3} {:.3}", point[0], point[1]));
    }
    path.push_str(" Z");
    path
}

fn append_polygon(geometry: &mut GpuGeometry, points: &[[f64; 2]], color: [f32; 4]) {
    let path = polygon_path(points);
    tessellate_path_into(geometry, &path, color);
}

fn append_circle(
    geometry: &mut GpuGeometry,
    center: lyon::math::Point,
    radius: f64,
    color: [f32; 4],
) {
    let mut points = Vec::with_capacity(CAP_SEGMENTS);
    for i in 0..CAP_SEGMENTS {
        let angle = std::f64::consts::TAU * i as f64 / CAP_SEGMENTS as f64;
        points.push([
            center.x as f64 + angle.cos() * radius,
            center.y as f64 + angle.sin() * radius,
        ]);
    }
    append_polygon(geometry, &points, color);
}

fn polygon_path(points: &[[f64; 2]]) -> Path {
    let mut builder = Path::builder();
    if let Some(first) = points.first() {
        builder.begin(lyon_point(first[0] as f32, first[1] as f32));
        for point in &points[1..] {
            builder.line_to(lyon_point(point[0] as f32, point[1] as f32));
        }
        builder.end(true);
    }
    builder.build()
}

fn tessellate_path_into(geometry: &mut GpuGeometry, path: &Path, color: [f32; 4]) {
    let mut tessellator = FillTessellator::new();
    let constructor = StrokeVertexConstructor { color };
    let mut buffers = VertexBuffers::<GpuVertex, u32>::new();
    let mut builder = BuffersBuilder::new(&mut buffers, constructor);
    let options = FillOptions::default().with_fill_rule(FillRule::NonZero);
    let _ = tessellator.tessellate_path(path, &options, &mut builder);

    let vertex_offset = geometry.vertices.len() as u32;
    geometry.vertices.extend(buffers.vertices);
    geometry.indices.extend(
        buffers
            .indices
            .into_iter()
            .map(|index| index + vertex_offset),
    );
}

#[derive(Clone, Debug)]
struct StrokeVertexConstructor {
    color: [f32; 4],
}

impl FillVertexConstructor<GpuVertex> for StrokeVertexConstructor {
    fn new_vertex(&mut self, vertex: FillVertex) -> GpuVertex {
        let position = vertex.position();
        GpuVertex {
            position: [position.x, position.y],
            color: self.color,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::StrokePoint;

    fn point(x: f32, y: f32) -> StrokePoint {
        StrokePoint::new(x, y, 0.55)
    }

    fn stroke_with_points(points: Vec<StrokePoint>) -> Stroke {
        Stroke {
            id: 1,
            points,
            color: [1.0, 0.0, 0.0, 1.0],
            width_scale: 1.0,
        }
    }

    #[test]
    fn tessellates_single_point_into_a_filled_circle() {
        let stroke = stroke_with_points(vec![point(10.0, 10.0)]);
        let geometry = tessellate_stroke(&stroke);

        assert!(!geometry.vertices.is_empty());
        assert!(!geometry.indices.is_empty());
        assert_eq!(geometry.indices.len() % 3, 0);
    }

    #[test]
    fn pressure_widens_the_stroke_but_never_below_a_quarter() {
        let stroke = stroke_with_points(vec![point(0.0, 0.0)]);

        assert!(width_for_pressure(&stroke, 1.0) > width_for_pressure(&stroke, 0.0));
        assert_eq!(width_for_pressure(&stroke, 0.0), BASE_WIDTH * 0.25);
    }

    #[test]
    fn svg_export_uses_closed_filled_outline() {
        let stroke = stroke_with_points(vec![point(0.0, 0.0), point(40.0, 10.0)]);
        let svg = stroke_svg_path(&stroke);

        assert!(svg.starts_with("M "));
        assert!(svg.contains(" L "));
        assert!(svg.ends_with(" Z"));
    }

    #[test]
    fn self_intersecting_stroke_keeps_crossing_area_filled() {
        let stroke = stroke_with_points(vec![
            StrokePoint::new(0.0, 0.0, 0.55),
            StrokePoint::new(50.0, 50.0, 0.55),
            StrokePoint::new(0.0, 50.0, 0.55),
            StrokePoint::new(50.0, 0.0, 0.55),
        ]);

        let geometry = tessellate_stroke(&stroke);
        assert!(!geometry.indices.is_empty());
        assert_eq!(geometry.indices.len() % 3, 0);

        let crossing = [25.0, 24.0];
        assert!(point_is_covered(&geometry, crossing));
    }

    #[test]
    fn self_intersecting_polygon_uses_nonzero_fill_rule() {
        let mut geometry = GpuGeometry::default();
        let polygon = [[0.0, 0.0], [20.0, 20.0], [0.0, 20.0], [20.0, 0.0]];

        append_polygon(&mut geometry, &polygon, [1.0, 0.0, 0.0, 1.0]);

        assert!(!geometry.indices.is_empty());
        assert!(point_is_covered(&geometry, [10.0, 10.0]));
    }

    fn point_is_covered(geometry: &GpuGeometry, point: [f32; 2]) -> bool {
        for triangle in geometry.indices.chunks_exact(3) {
            let a = geometry.vertices[triangle[0] as usize].position;
            let b = geometry.vertices[triangle[1] as usize].position;
            let c = geometry.vertices[triangle[2] as usize].position;
            if point_in_triangle(point, a, b, c) {
                return true;
            }
        }
        false
    }

    fn sign(p1: [f32; 2], p2: [f32; 2], p3: [f32; 2]) -> f32 {
        (p1[0] - p3[0]) * (p2[1] - p3[1]) - (p2[0] - p3[0]) * (p1[1] - p3[1])
    }

    fn point_in_triangle(pt: [f32; 2], a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> bool {
        let d1 = sign(pt, a, b);
        let d2 = sign(pt, b, c);
        let d3 = sign(pt, c, a);
        let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
        let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
        !(has_neg && has_pos)
    }
}
