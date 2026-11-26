//! Angular plates: a small UI material (`assets/ui/shaders/plate.wesl`) that
//! draws convex shapes with chamfered corners and slanted sides, a fill
//! gradient and an edge line, crisp at any scale (Bevy UI has no
//! clip-path). The node's border widths are the edge line, its border radii
//! are the corner cuts (both scale with the interface); slants are insets
//! as fractions of the node's height.
//!
//! Identical static plates share one material (`PlateCache`); plates that
//! animate (`PlateAnim`) own theirs and are rewritten only when their
//! colours change.

use std::collections::HashMap;

use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;
use bevy::ui_render::prelude::{MaterialNode, UiMaterial, UiMaterialPlugin};

use super::theme as t;

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct PlateMaterial {
    #[uniform(0)]
    pub fill_top: LinearRgba,
    #[uniform(0)]
    pub fill_bottom: LinearRgba,
    #[uniform(0)]
    pub line: LinearRgba,
    #[uniform(0)]
    pub slant: Vec4,
    #[uniform(0)]
    pub extra: Vec4,
}

impl UiMaterial for PlateMaterial {
    fn fragment_shader() -> ShaderRef {
        "ui/shaders/plate.wesl".into()
    }
}

/// How a plate looks. Shape (cuts, line widths) comes from the node.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlateStyle {
    pub fill_top: Color,
    pub fill_bottom: Color,
    pub line: Color,
    /// Left top / left bottom / right top / right bottom insets (fractions of
    /// the height).
    pub slant: [f32; 4],
    /// Hatching strength (0 = none).
    pub hatch: f32,
    /// Lighter band under the top edge.
    pub sheen: f32,
}

impl PlateStyle {
    /// A docked gunmetal plate with the lighter edge line.
    pub const PANEL: PlateStyle = PlateStyle {
        fill_top: t::PLATE_TOP,
        fill_bottom: t::PLATE_BOTTOM,
        line: t::LINE,
        slant: [0.0; 4],
        hatch: 0.0,
        sheen: 0.008,
    };

    pub const fn fill(top: Color, bottom: Color, line: Color) -> PlateStyle {
        PlateStyle {
            fill_top: top,
            fill_bottom: bottom,
            line,
            slant: [0.0; 4],
            hatch: 0.0,
            sheen: 0.0,
        }
    }

    pub const fn slanted(mut self, slant: [f32; 4]) -> PlateStyle {
        self.slant = slant;
        self
    }

    pub const fn hatched(mut self, hatch: f32) -> PlateStyle {
        self.hatch = hatch;
        self
    }

    fn material(&self) -> PlateMaterial {
        PlateMaterial {
            fill_top: self.fill_top.to_linear(),
            fill_bottom: self.fill_bottom.to_linear(),
            line: self.line.to_linear(),
            slant: Vec4::from_array(self.slant),
            extra: Vec4::new(self.hatch, 0.16, self.sheen, 0.0),
        }
    }

    fn key(&self) -> [u32; 18] {
        let c = |c: Color| c.to_srgba().to_f32_array().map(f32::to_bits);
        let [a, b, cc, d] = c(self.fill_top);
        let [e, f, g, h] = c(self.fill_bottom);
        let [i, j, k, l] = c(self.line);
        let s = self.slant.map(f32::to_bits);
        [
            a,
            b,
            cc,
            d,
            e,
            f,
            g,
            h,
            i,
            j,
            k,
            l,
            s[0],
            s[1],
            s[2],
            s[3],
            self.hatch.to_bits(),
            self.sheen.to_bits(),
        ]
    }
}

/// Shared materials of static plates.
#[derive(Resource, Default)]
pub struct PlateCache(HashMap<[u32; 18], Handle<PlateMaterial>>);

impl PlateCache {
    pub fn get(&mut self, mats: &mut Assets<PlateMaterial>, style: PlateStyle) -> MaterialNode<PlateMaterial> {
        MaterialNode(
            self.0
                .entry(style.key())
                .or_insert_with(|| mats.add(style.material()))
                .clone(),
        )
    }
}

/// A plate whose look changes (buttons drawn as plates): it owns its
/// material; set `style` and the material follows.
#[derive(Component, Debug, Clone, Copy)]
pub struct PlateAnim {
    pub style: PlateStyle,
    shown: Option<PlateStyle>,
}

impl PlateAnim {
    pub fn new(style: PlateStyle) -> PlateAnim {
        PlateAnim { style, shown: None }
    }
}

pub struct PlatePlugin;

impl Plugin for PlatePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(UiMaterialPlugin::<PlateMaterial>::default())
            .init_resource::<PlateCache>()
            .add_systems(PostUpdate, (own_materials, sync_materials).chain());
    }
}

/// Give a new animated plate its own material.
fn own_materials(
    mut commands: Commands,
    mut mats: ResMut<Assets<PlateMaterial>>,
    q: Query<(Entity, &PlateAnim), Without<MaterialNode<PlateMaterial>>>,
) {
    for (e, anim) in &q {
        commands.entity(e).insert(MaterialNode(mats.add(anim.style.material())));
    }
}

fn sync_materials(
    mut mats: ResMut<Assets<PlateMaterial>>,
    mut q: Query<(&mut PlateAnim, &MaterialNode<PlateMaterial>), Changed<PlateAnim>>,
) {
    for (mut anim, node) in &mut q {
        if anim.shown == Some(anim.style) {
            continue;
        }
        let style = anim.style;
        anim.bypass_change_detection().shown = Some(style);
        if let Some(mut m) = mats.get_mut(&node.0) {
            *m = style.material();
        }
    }
}
