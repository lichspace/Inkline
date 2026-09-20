use rstar::{PointDistance, RTree, RTreeObject, AABB};

use crate::model::{Document, LayerId, StrokeId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StrokeRef {
    pub layer_id: LayerId,
    pub stroke_id: StrokeId,
}

#[derive(Clone, Debug)]
struct IndexedStroke {
    reference: StrokeRef,
    bounds: [f32; 4],
}

impl RTreeObject for IndexedStroke {
    type Envelope = AABB<[f32; 2]>;

    fn envelope(&self) -> Self::Envelope {
        AABB::from_corners(
            [self.bounds[0], self.bounds[1]],
            [self.bounds[2], self.bounds[3]],
        )
    }
}

impl PointDistance for IndexedStroke {
    fn distance_2(&self, point: &[f32; 2]) -> f32 {
        let dx = if point[0] < self.bounds[0] {
            self.bounds[0] - point[0]
        } else if point[0] > self.bounds[2] {
            point[0] - self.bounds[2]
        } else {
            0.0
        };
        let dy = if point[1] < self.bounds[1] {
            self.bounds[1] - point[1]
        } else if point[1] > self.bounds[3] {
            point[1] - self.bounds[3]
        } else {
            0.0
        };
        dx * dx + dy * dy
    }
}

fn build_index(document: &Document) -> RTree<IndexedStroke> {
    let mut items = Vec::new();
    for layer in &document.layers {
        if !layer.visible {
            continue;
        }
        for stroke in &layer.strokes {
            if let Some(bounds) = stroke.bounds() {
                let padding = 8.0;
                items.push(IndexedStroke {
                    reference: StrokeRef {
                        layer_id: layer.id,
                        stroke_id: stroke.id,
                    },
                    bounds: [
                        bounds[0] - padding,
                        bounds[1] - padding,
                        bounds[2] + padding,
                        bounds[3] + padding,
                    ],
                });
            }
        }
    }
    RTree::bulk_load(items)
}

pub fn hit_test(document: &Document, point: [f32; 2], max_distance: f32) -> Vec<StrokeRef> {
    let index = build_index(document);
    index
        .locate_within_distance(point, max_distance * max_distance)
        .map(|item| item.reference)
        .collect()
}

pub fn rect_select(document: &Document, min: [f32; 2], max: [f32; 2]) -> Vec<StrokeRef> {
    let index = build_index(document);
    let envelope = AABB::from_corners(min, max);
    index
        .locate_in_envelope_intersecting(envelope)
        .map(|item| item.reference)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Stroke, StrokePoint};

    #[test]
    fn hit_test_finds_strokes_near_the_pointer() {
        let mut document = Document::new(100.0, 100.0);
        document.layer_mut(1).unwrap().strokes.push(Stroke {
            id: 10,
            points: vec![
                StrokePoint::new(10.0, 10.0, 1.0),
                StrokePoint::new(30.0, 30.0, 1.0),
            ],
            color: [0.0, 0.0, 0.0, 1.0],
            width_scale: 1.0,
        });

        let hits = hit_test(&document, [20.0, 20.0], 3.0);

        assert!(hits.iter().any(|item| item.stroke_id == 10));
    }

    #[test]
    fn rect_select_finds_strokes_within_a_box() {
        let mut document = Document::new(100.0, 100.0);
        document.layer_mut(1).unwrap().strokes.push(Stroke {
            id: 20,
            points: vec![
                StrokePoint::new(40.0, 40.0, 1.0),
                StrokePoint::new(60.0, 60.0, 1.0),
            ],
            color: [0.0, 0.0, 0.0, 1.0],
            width_scale: 1.0,
        });

        let hits = rect_select(&document, [30.0, 30.0], [70.0, 70.0]);

        assert!(hits.iter().any(|item| item.stroke_id == 20));
    }
}
