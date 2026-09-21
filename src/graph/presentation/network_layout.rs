use std::collections::{BTreeMap, HashMap, VecDeque};

use serde::Serialize;

use crate::graph::{artifact::GraphArtifact, model::NodeKey};

const SPACING: f64 = 100.0;
const COMPONENT_MARGIN: f64 = 100.0;
const EXACT_REPULSION_LIMIT: usize = 128;
const EXACT_ITERATIONS: usize = 120;
const LOCAL_ITERATIONS: usize = 48;
const CELL_SIZE: f64 = SPACING * 2.0;
const CELL_SAMPLES: usize = 8;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub(super) struct NetworkPosition {
    x: f64,
    y: f64,
}

/// Presentation coordinates are independent of operational layers and hashes.
/// Small components use pairwise repulsion. Large components use a fixed number
/// of samples in nine nearby cells, bounding each pass by O(nodes + edges).
pub(super) fn build(artifact: &GraphArtifact) -> BTreeMap<NodeKey, NetworkPosition> {
    let keys: Vec<_> = artifact
        .nodes
        .iter()
        .map(|node| node.key().clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let indices: BTreeMap<_, _> = keys
        .iter()
        .enumerate()
        .map(|(index, key)| (key, index))
        .collect();
    let mut neighbors = vec![Vec::new(); keys.len()];
    for edge in &artifact.edges {
        if let (Some(&left), Some(&right)) =
            (indices.get(&edge.blocker), indices.get(&edge.blocked))
            && left != right
        {
            neighbors[left].push(right);
            neighbors[right].push(left);
        }
    }
    for adjacent in &mut neighbors {
        adjacent.sort_unstable();
        adjacent.dedup();
    }
    let components = components(&neighbors);
    let mut layouts = Vec::with_capacity(components.len());
    let mut local_indices = vec![0; keys.len()];
    for members in components {
        for (local, &global) in members.iter().enumerate() {
            local_indices[global] = local;
        }
        let mut edges = Vec::new();
        for (local, &global) in members.iter().enumerate() {
            for &neighbor in &neighbors[global] {
                if global < neighbor {
                    edges.push((local, local_indices[neighbor]));
                }
            }
        }
        edges.sort_unstable();
        let (positions, _) = relax(members.len(), &edges);
        layouts.push(ComponentLayout::new(members, positions));
    }
    pack(&mut layouts);
    layouts
        .into_iter()
        .flat_map(|layout| {
            layout
                .members
                .into_iter()
                .zip(layout.positions)
                .map(|(index, position)| (keys[index].clone(), position))
        })
        .collect()
}

fn components(neighbors: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let mut visited = vec![false; neighbors.len()];
    let mut result = Vec::new();
    for first in 0..neighbors.len() {
        if visited[first] {
            continue;
        }
        let mut queue = VecDeque::from([first]);
        let mut members = Vec::new();
        visited[first] = true;
        while let Some(current) = queue.pop_front() {
            members.push(current);
            for &neighbor in &neighbors[current] {
                if !visited[neighbor] {
                    visited[neighbor] = true;
                    queue.push_back(neighbor);
                }
            }
        }
        result.push(members);
    }
    result.sort_by_key(|members| (std::cmp::Reverse(members.len()), members[0]));
    result
}

#[derive(Default)]
struct LayoutWork {
    #[cfg(test)]
    repulsion_evaluations: usize,
    #[cfg(test)]
    spring_evaluations: usize,
}

impl LayoutWork {
    fn repulsion(&mut self) {
        #[cfg(test)]
        {
            self.repulsion_evaluations += 1;
        }
    }

    fn spring(&mut self) {
        #[cfg(test)]
        {
            self.spring_evaluations += 1;
        }
    }
}

fn relax(count: usize, edges: &[(usize, usize)]) -> (Vec<NetworkPosition>, LayoutWork) {
    let mut work = LayoutWork::default();
    let golden_angle = std::f64::consts::PI * (3.0 - 5.0_f64.sqrt());
    let mut positions: Vec<_> = (0..count)
        .map(|index| {
            let radius = SPACING * 0.7 * (index as f64).sqrt();
            let angle = golden_angle * index as f64;
            NetworkPosition {
                x: radius * angle.cos(),
                y: radius * angle.sin(),
            }
        })
        .collect();
    if count < 2 {
        return (positions, work);
    }
    let exact = count <= EXACT_REPULSION_LIMIT;
    let iterations = if exact {
        EXACT_ITERATIONS
    } else {
        LOCAL_ITERATIONS
    };
    let mut forces = vec![NetworkPosition::default(); count];
    for step in 0..iterations {
        forces.fill(NetworkPosition::default());
        if exact {
            for left in 0..count {
                for right in left + 1..count {
                    let force = repulsion(positions[left], positions[right], left, right);
                    forces[left].x += force.x;
                    forces[left].y += force.y;
                    forces[right].x -= force.x;
                    forces[right].y -= force.y;
                    work.repulsion();
                }
            }
        } else {
            local_repulsion(&positions, &mut forces, &mut work);
        }
        for &(left, right) in edges {
            let dx = positions[right].x - positions[left].x;
            let dy = positions[right].y - positions[left].y;
            let distance = dx.hypot(dy);
            let strength = (distance / SPACING).min(8.0) * 0.05;
            forces[left].x += dx * strength;
            forces[left].y += dy * strength;
            forces[right].x -= dx * strength;
            forces[right].y -= dy * strength;
            work.spring();
        }
        let temperature = 24.0 * (1.0 - step as f64 / iterations as f64) + 0.5;
        for (position, force) in positions.iter_mut().zip(&forces) {
            let length = force.x.hypot(force.y).max(0.001);
            let scale = (temperature / length).min(1.0);
            position.x += force.x * scale;
            position.y += force.y * scale;
        }
    }
    (positions, work)
}

fn repulsion(
    left: NetworkPosition,
    right: NetworkPosition,
    left_index: usize,
    right_index: usize,
) -> NetworkPosition {
    let mut dx = left.x - right.x;
    let mut dy = left.y - right.y;
    if dx.abs() + dy.abs() < 0.001 {
        // Break an exact overlap without randomness or dependence on map order.
        dx = if left_index < right_index { -1.0 } else { 1.0 };
        dy = dx * 0.5;
    }
    let scale = SPACING * SPACING * 0.05 / (dx * dx + dy * dy).max(1.0);
    NetworkPosition {
        x: dx * scale,
        y: dy * scale,
    }
}

#[derive(Default)]
struct Cell {
    count: usize,
    samples: Vec<usize>,
}

fn cell(position: NetworkPosition) -> (i64, i64) {
    (
        (position.x / CELL_SIZE).floor() as i64,
        (position.y / CELL_SIZE).floor() as i64,
    )
}

fn local_repulsion(
    positions: &[NetworkPosition],
    forces: &mut [NetworkPosition],
    work: &mut LayoutWork,
) {
    let mut cells = HashMap::<(i64, i64), Cell>::new();
    for (index, &position) in positions.iter().enumerate() {
        let entry = cells.entry(cell(position)).or_default();
        entry.count += 1;
        if entry.samples.len() < CELL_SAMPLES {
            entry.samples.push(index);
        }
    }
    // Hash table iteration never affects the layout: both cell lookups and
    // samples follow a fixed order, including when a cell exceeds its budget.
    for (left, &position) in positions.iter().enumerate() {
        let (x, y) = cell(position);
        for dx in -1..=1 {
            for dy in -1..=1 {
                if let Some(neighbors) = cells.get(&(x + dx, y + dy)) {
                    let weight = neighbors.count as f64 / neighbors.samples.len() as f64;
                    for &right in &neighbors.samples {
                        if left != right {
                            let force = repulsion(position, positions[right], left, right);
                            forces[left].x += force.x * weight;
                            forces[left].y += force.y * weight;
                            work.repulsion();
                        }
                    }
                }
            }
        }
    }
}

struct ComponentLayout {
    members: Vec<usize>,
    positions: Vec<NetworkPosition>,
    width: f64,
    height: f64,
}

impl ComponentLayout {
    fn new(members: Vec<usize>, mut positions: Vec<NetworkPosition>) -> Self {
        let min_x = positions
            .iter()
            .map(|point| point.x)
            .fold(f64::INFINITY, f64::min);
        let min_y = positions
            .iter()
            .map(|point| point.y)
            .fold(f64::INFINITY, f64::min);
        let max_x = positions
            .iter()
            .map(|point| point.x)
            .fold(f64::NEG_INFINITY, f64::max);
        let max_y = positions
            .iter()
            .map(|point| point.y)
            .fold(f64::NEG_INFINITY, f64::max);
        for point in &mut positions {
            point.x -= min_x;
            point.y -= min_y;
        }
        Self {
            members,
            positions,
            width: (max_x - min_x).max(SPACING * 0.5) + COMPONENT_MARGIN,
            height: (max_y - min_y).max(SPACING * 0.5) + COMPONENT_MARGIN,
        }
    }
}

fn pack(layouts: &mut [ComponentLayout]) {
    let area: f64 = layouts
        .iter()
        .map(|layout| layout.width * layout.height)
        .sum();
    let widest = layouts
        .iter()
        .map(|layout| layout.width)
        .fold(0.0, f64::max);
    let target_width = area.sqrt().max(widest);
    let (mut x, mut y, mut row_height) = (0.0, 0.0, 0.0_f64);
    for layout in layouts {
        if x > 0.0 && x + layout.width > target_width {
            x = 0.0;
            y += row_height;
            row_height = 0.0;
        }
        for point in &mut layout.positions {
            point.x += x + COMPONENT_MARGIN / 2.0;
            point.y += y + COMPONENT_MARGIN / 2.0;
            // Avoid insignificant platform-specific trigonometric tails in JSON.
            point.x = (point.x * 100.0).round() / 100.0;
            point.y = (point.y * 100.0).round() / 100.0;
        }
        x += layout.width;
        row_height = row_height.max(layout.height);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_component_has_linear_bounded_work_and_finite_positions() {
        let count = 10_000;
        let edges: Vec<_> = (0..count)
            .flat_map(|left| (1..=4).map(move |offset| (left, (left + offset) % count)))
            .collect();
        let (positions, work) = relax(count, &edges);
        assert_eq!(positions.len(), count);
        assert!(
            positions
                .iter()
                .all(|point| point.x.is_finite() && point.y.is_finite())
        );
        assert!(work.repulsion_evaluations <= LOCAL_ITERATIONS * count * 9 * CELL_SAMPLES);
        assert_eq!(work.spring_evaluations, LOCAL_ITERATIONS * edges.len());
        assert!(work.repulsion_evaluations < count * count);
    }

    #[test]
    fn overloaded_cells_still_have_bounded_repulsion_and_separate_overlaps() {
        let count = 1_000;
        let positions = vec![NetworkPosition::default(); count];
        let mut forces = vec![NetworkPosition::default(); count];
        let mut work = LayoutWork::default();
        local_repulsion(&positions, &mut forces, &mut work);
        assert!(work.repulsion_evaluations <= count * CELL_SAMPLES);
        assert!(
            forces
                .iter()
                .all(|point| point.x.is_finite() && point.y.is_finite())
        );
        assert!(forces.first().unwrap().x < 0.0);
        assert!(forces.last().unwrap().x > 0.0);
    }

    #[test]
    fn cycle_forms_a_separated_spatial_component_deterministically() {
        let edges = [(0, 1), (1, 2), (2, 3), (3, 0)];
        let (first, _) = relax(4, &edges);
        let (second, _) = relax(4, &edges);
        assert_eq!(first, second);
        for left in 0..first.len() {
            for right in left + 1..first.len() {
                let distance =
                    (first[left].x - first[right].x).hypot(first[left].y - first[right].y);
                assert!(distance > 30.0, "nodes must remain separately selectable");
            }
        }
        let width = first
            .iter()
            .map(|point| point.x)
            .fold(f64::NEG_INFINITY, f64::max)
            - first
                .iter()
                .map(|point| point.x)
                .fold(f64::INFINITY, f64::min);
        let height = first
            .iter()
            .map(|point| point.y)
            .fold(f64::NEG_INFINITY, f64::max)
            - first
                .iter()
                .map(|point| point.y)
                .fold(f64::INFINITY, f64::min);
        assert!(width > 50.0 && height > 50.0);
    }
}
