use serde::{Deserialize, Serialize};

use crate::model::{Document, Layer, LayerId, Stroke};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Command {
    AddStroke {
        layer_id: LayerId,
        stroke: Stroke,
        index: usize,
    },
    RemoveStrokes {
        layer_id: LayerId,
        strokes: Vec<Stroke>,
        indices: Vec<usize>,
    },
    ModifyStrokes {
        layer_id: LayerId,
        before: Vec<Stroke>,
        after: Vec<Stroke>,
    },
    AddLayer {
        layer: Layer,
        index: usize,
        previous_active_layer_id: LayerId,
        new_active_layer_id: LayerId,
    },
    RemoveLayer {
        layer: Layer,
        index: usize,
        previous_active_layer_id: LayerId,
        new_active_layer_id: LayerId,
    },
    ReorderLayer {
        from: usize,
        to: usize,
    },
    SetLayer {
        before: Layer,
        after: Layer,
    },
    SetBackgroundColor {
        before: [f32; 4],
        after: [f32; 4],
    },
}

impl Command {
    pub fn apply(&self, document: &mut Document) {
        match self {
            Self::AddStroke {
                layer_id,
                stroke,
                index,
            } => {
                if let Some(layer) = document.layer_mut(*layer_id) {
                    let index = (*index).min(layer.strokes.len());
                    layer.strokes.insert(index, stroke.clone());
                }
            }
            Self::RemoveStrokes {
                layer_id, strokes, ..
            } => {
                if let Some(layer) = document.layer_mut(*layer_id) {
                    let ids: std::collections::HashSet<_> =
                        strokes.iter().map(|stroke| stroke.id).collect();
                    layer.strokes.retain(|stroke| !ids.contains(&stroke.id));
                }
            }
            Self::ModifyStrokes {
                layer_id, after, ..
            } => {
                if let Some(layer) = document.layer_mut(*layer_id) {
                    for replacement in after {
                        if let Some(stroke) = layer
                            .strokes
                            .iter_mut()
                            .find(|stroke| stroke.id == replacement.id)
                        {
                            *stroke = replacement.clone();
                        }
                    }
                }
            }
            Self::AddLayer {
                layer,
                index,
                previous_active_layer_id: _,
                new_active_layer_id,
            } => {
                let index = (*index).min(document.layers.len());
                document.layers.insert(index, layer.clone());
                document.active_layer_id = *new_active_layer_id;
            }
            Self::RemoveLayer {
                layer,
                index,
                previous_active_layer_id: _,
                new_active_layer_id,
            } => {
                let index = (*index).min(document.layers.len().saturating_sub(1));
                if document.layers.get(index).map(|item| item.id) == Some(layer.id) {
                    document.layers.remove(index);
                } else {
                    document.layers.retain(|item| item.id != layer.id);
                }
                document.active_layer_id = *new_active_layer_id;
            }
            Self::ReorderLayer { from, to } => {
                if document.layers.is_empty() || *from >= document.layers.len() {
                    return;
                }
                let layer = document.layers.remove(*from);
                let to = (*to).min(document.layers.len());
                document.layers.insert(to, layer);
            }
            Self::SetLayer { after, .. } => {
                if let Some(layer) = document.layer_mut(after.id) {
                    *layer = after.clone();
                }
            }
            Self::SetBackgroundColor { after, .. } => {
                document.background_color = *after;
            }
        }
    }

    pub fn revert(&self, document: &mut Document) {
        match self {
            Self::AddStroke {
                layer_id, stroke, ..
            } => {
                if let Some(layer) = document.layer_mut(*layer_id) {
                    layer.strokes.retain(|item| item.id != stroke.id);
                }
            }
            Self::RemoveStrokes {
                layer_id,
                strokes,
                indices,
            } => {
                if let Some(layer) = document.layer_mut(*layer_id) {
                    let mut pairs: Vec<_> = indices
                        .iter()
                        .cloned()
                        .zip(strokes.iter().cloned())
                        .collect();
                    pairs.sort_by_key(|(index, _)| *index);
                    for (index, stroke) in pairs {
                        let index = index.min(layer.strokes.len());
                        layer.strokes.insert(index, stroke);
                    }
                }
            }
            Self::ModifyStrokes {
                layer_id, before, ..
            } => {
                if let Some(layer) = document.layer_mut(*layer_id) {
                    for replacement in before {
                        if let Some(stroke) = layer
                            .strokes
                            .iter_mut()
                            .find(|stroke| stroke.id == replacement.id)
                        {
                            *stroke = replacement.clone();
                        }
                    }
                }
            }
            Self::AddLayer {
                layer,
                previous_active_layer_id,
                ..
            } => {
                document.layers.retain(|item| item.id != layer.id);
                document.active_layer_id = *previous_active_layer_id;
            }
            Self::RemoveLayer {
                layer,
                index,
                previous_active_layer_id,
                ..
            } => {
                let index = (*index).min(document.layers.len());
                document.layers.insert(index, layer.clone());
                document.active_layer_id = *previous_active_layer_id;
            }
            Self::ReorderLayer { from, to } => {
                if document.layers.is_empty() || *to >= document.layers.len() {
                    return;
                }
                let layer = document.layers.remove(*to);
                let from = (*from).min(document.layers.len());
                document.layers.insert(from, layer);
            }
            Self::SetLayer { before, .. } => {
                if let Some(layer) = document.layer_mut(before.id) {
                    *layer = before.clone();
                }
            }
            Self::SetBackgroundColor { before, .. } => {
                document.background_color = *before;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::StrokePoint;

    fn stroke(id: u64) -> Stroke {
        Stroke {
            id,
            points: vec![StrokePoint::new(0.0, 0.0, 1.0)],
            color: [0.0, 0.0, 0.0, 1.0],
            width_scale: 1.0,
        }
    }

    #[test]
    fn add_and_revert_stroke_round_trip() {
        let mut document = Document::new(100.0, 100.0);
        let command = Command::AddStroke {
            layer_id: 1,
            stroke: stroke(7),
            index: 0,
        };

        command.apply(&mut document);
        assert_eq!(document.layer(1).unwrap().strokes.len(), 1);

        command.revert(&mut document);
        assert!(document.layer(1).unwrap().strokes.is_empty());
    }

    #[test]
    fn modify_strokes_applies_and_reverts() {
        let mut document = Document::new(100.0, 100.0);
        document.layer_mut(1).unwrap().strokes.push(stroke(1));

        let before = document.layer(1).unwrap().strokes.clone();
        let after = vec![Stroke {
            color: [1.0, 0.0, 0.0, 1.0],
            ..stroke(1)
        }];
        let command = Command::ModifyStrokes {
            layer_id: 1,
            before,
            after,
        };

        command.apply(&mut document);
        assert_eq!(document.layer(1).unwrap().strokes[0].color[0], 1.0);

        command.revert(&mut document);
        assert_eq!(document.layer(1).unwrap().strokes[0].color[0], 0.0);
    }

    #[test]
    fn reorder_layer_is_reversible() {
        let mut document = Document::new(100.0, 100.0);
        document.next_layer_id = 3;
        document.layers.push(Layer::new(2, "Layer 2"));
        let original = document.layers.clone();
        let command = Command::ReorderLayer { from: 0, to: 1 };

        command.apply(&mut document);
        assert_eq!(document.layers[0].id, 2);

        command.revert(&mut document);
        assert_eq!(document.layers, original);
    }

    #[test]
    fn background_color_change_is_reversible() {
        let mut document = Document::new(100.0, 100.0);
        let before = document.background_color;
        let after = [0.2, 0.4, 0.6, 0.8];
        let command = Command::SetBackgroundColor { before, after };

        command.apply(&mut document);
        assert_eq!(document.background_color, after);

        command.revert(&mut document);
        assert_eq!(document.background_color, before);
    }
}
