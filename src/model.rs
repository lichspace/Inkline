use serde::{Deserialize, Serialize};

pub type LayerId = u64;
pub type StrokeId = u64;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
    Add,
    Subtract,
}

impl BlendMode {
    pub const ALL: [Self; 5] = [
        Self::Normal,
        Self::Multiply,
        Self::Screen,
        Self::Add,
        Self::Subtract,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Multiply => "Multiply",
            Self::Screen => "Screen",
            Self::Add => "Add",
            Self::Subtract => "Subtract",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrokePoint {
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
}

impl StrokePoint {
    pub fn new(x: f32, y: f32, pressure: f32) -> Self {
        Self { x, y, pressure }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    pub id: StrokeId,
    pub points: Vec<StrokePoint>,
    pub color: [f32; 4],
    pub width_scale: f32,
}

#[allow(dead_code)]
impl Stroke {
    pub fn new(id: StrokeId, points: Vec<StrokePoint>, color: [f32; 4]) -> Self {
        Self {
            id,
            points,
            color,
            width_scale: 1.0,
        }
    }

    pub fn bounds(&self) -> Option<[f32; 4]> {
        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;

        for point in &self.points {
            min_x = min_x.min(point.x);
            min_y = min_y.min(point.y);
            max_x = max_x.max(point.x);
            max_y = max_y.max(point.y);
        }

        if self.points.is_empty() {
            None
        } else {
            Some([min_x, min_y, max_x, max_y])
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub id: LayerId,
    pub name: String,
    pub visible: bool,
    pub opacity: f32,
    pub blend_mode: BlendMode,
    pub strokes: Vec<Stroke>,
}

impl Layer {
    pub fn new(id: LayerId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            visible: true,
            opacity: 1.0,
            blend_mode: BlendMode::Normal,
            strokes: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub width: f32,
    pub height: f32,
    #[serde(default = "default_background_color")]
    pub background_color: [f32; 4],
    pub layers: Vec<Layer>,
    pub active_layer_id: LayerId,
    pub next_layer_id: LayerId,
    pub next_stroke_id: StrokeId,
}

pub fn default_background_color() -> [f32; 4] {
    [1.0, 1.0, 1.0, 1.0]
}

#[allow(dead_code)]
impl Document {
    pub fn new(width: f32, height: f32) -> Self {
        let layers = vec![Layer::new(1, "Layer 1")];

        Self {
            width,
            height,
            background_color: default_background_color(),
            active_layer_id: 1,
            layers,
            next_layer_id: 2,
            next_stroke_id: 1,
        }
    }

    pub fn layer_index(&self, layer_id: LayerId) -> Option<usize> {
        self.layers.iter().position(|layer| layer.id == layer_id)
    }

    pub fn layer(&self, layer_id: LayerId) -> Option<&Layer> {
        self.layers.iter().find(|layer| layer.id == layer_id)
    }

    pub fn layer_mut(&mut self, layer_id: LayerId) -> Option<&mut Layer> {
        self.layers.iter_mut().find(|layer| layer.id == layer_id)
    }

    pub fn active_layer(&self) -> &Layer {
        self.layer(self.active_layer_id)
            .expect("active layer must exist")
    }

    pub fn active_layer_mut(&mut self) -> &mut Layer {
        self.layer_mut(self.active_layer_id)
            .expect("active layer must exist")
    }

    pub fn allocate_stroke_id(&mut self) -> StrokeId {
        let id = self.next_stroke_id;
        self.next_stroke_id += 1;
        id
    }

    pub fn allocate_layer_id(&mut self) -> LayerId {
        let id = self.next_layer_id;
        self.next_layer_id += 1;
        id
    }

    pub fn add_layer(&mut self) -> LayerId {
        let id = self.allocate_layer_id();
        self.layers
            .push(Layer::new(id, format!("Layer {}", self.layers.len() + 1)));
        self.active_layer_id = id;
        id
    }

    pub fn remove_layer(&mut self, layer_id: LayerId) -> Option<(usize, Layer)> {
        let index = self.layer_index(layer_id)?;
        let layer = self.layers.remove(index);

        if self.active_layer_id == layer_id {
            self.active_layer_id = self
                .layers
                .get(index.min(self.layers.len().saturating_sub(1)))
                .map(|candidate| candidate.id)
                .unwrap_or(0);
        }

        Some((index, layer))
    }

    pub fn add_stroke_to_layer(&mut self, layer_id: LayerId, mut stroke: Stroke) -> Option<usize> {
        if stroke.id == 0 {
            stroke.id = self.allocate_stroke_id();
        }
        let layer = self.layer_mut(layer_id)?;
        let index = layer.strokes.len();
        layer.strokes.push(stroke);
        Some(index)
    }

    pub fn find_stroke_mut(
        &mut self,
        layer_id: LayerId,
        stroke_id: StrokeId,
    ) -> Option<&mut Stroke> {
        self.layer_mut(layer_id)?
            .strokes
            .iter_mut()
            .find(|stroke| stroke.id == stroke_id)
    }

    pub fn find_stroke(&self, layer_id: LayerId, stroke_id: StrokeId) -> Option<&Stroke> {
        self.layer(layer_id)?
            .strokes
            .iter()
            .find(|stroke| stroke.id == stroke_id)
    }
}
