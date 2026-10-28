use std::collections::HashSet;

/// Identifies a render resource by a static name.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ResourceId(pub &'static str);

impl ResourceId {
    pub const SHADOW_DEPTH: Self = Self("shadow_depth");
    pub const SCENE: Self = Self("scene");
    pub const SCENE_COPY: Self = Self("scene_copy");
    pub const NORMAL: Self = Self("normal");
    pub const DEPTH: Self = Self("depth");
    pub const PROCESSED: Self = Self("processed");
    pub const SURFACE: Self = Self("surface");
}

/// Declares what a pass reads and writes.
pub struct PassDecl {
    pub name: &'static str,
    pub reads: &'static [ResourceId],
    pub writes: &'static [ResourceId],
}

/// Maps ResourceId -> TextureView reference for graph execution.
pub struct ResourceMap<'a> {
    entries: [Option<(&'static str, &'a wgpu::TextureView)>; 8],
    len: usize,
}

impl<'a> ResourceMap<'a> {
    pub fn new() -> Self {
        Self {
            entries: [None; 8],
            len: 0,
        }
    }

    pub fn insert(&mut self, id: ResourceId, view: &'a wgpu::TextureView) {
        // Overwrite if exists
        for entry in &mut self.entries[..self.len] {
            if let Some((name, ref mut v)) = entry {
                if *name == id.0 {
                    *v = view;
                    return;
                }
            }
        }
        assert!(self.len < 8, "ResourceMap overflow");
        self.entries[self.len] = Some((id.0, view));
        self.len += 1;
    }

    pub fn get(&self, id: ResourceId) -> &'a wgpu::TextureView {
        for entry in &self.entries[..self.len] {
            if let Some((name, view)) = entry {
                if *name == id.0 {
                    return view;
                }
            }
        }
        panic!("ResourceMap: {:?} not found", id);
    }
}

/// Trait for render passes that participate in the graph.
pub trait RenderPassNode {
    fn declaration(&self) -> PassDecl;
    /// Record the pass, returning how many draws it issued.
    fn record(&self, encoder: &mut wgpu::CommandEncoder, resources: &ResourceMap) -> u32;
}

/// A render pass that counts the draws recorded into it, for the Performance
/// panel's draw count. It derefs to the wrapped `wgpu::RenderPass`, so a node
/// records through it unchanged; only `draw` and `draw_indexed` are counted. A
/// node that draws any other way (indirect, multi-draw) is not counted.
pub struct CountingPass<'e> {
    pass: wgpu::RenderPass<'e>,
    draws: u32,
}

impl<'e> CountingPass<'e> {
    pub fn new(pass: wgpu::RenderPass<'e>) -> Self {
        Self { pass, draws: 0 }
    }

    pub fn draw(&mut self, vertices: std::ops::Range<u32>, instances: std::ops::Range<u32>) {
        self.pass.draw(vertices, instances);
        self.draws += 1;
    }

    pub fn draw_indexed(
        &mut self,
        indices: std::ops::Range<u32>,
        base_vertex: i32,
        instances: std::ops::Range<u32>,
    ) {
        self.pass.draw_indexed(indices, base_vertex, instances);
        self.draws += 1;
    }

    /// Draws recorded so far.
    pub fn draws(&self) -> u32 {
        self.draws
    }
}

impl<'e> std::ops::Deref for CountingPass<'e> {
    type Target = wgpu::RenderPass<'e>;
    fn deref(&self) -> &Self::Target { &self.pass }
}

impl std::ops::DerefMut for CountingPass<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.pass }
}

/// Collects passes and executes them in order.
pub struct RenderGraph<'a> {
    passes: Vec<&'a dyn RenderPassNode>,
}

impl<'a> RenderGraph<'a> {
    pub fn new() -> Self {
        Self {
            passes: Vec::with_capacity(8),
        }
    }
    
    pub fn add_pass(&mut self, pass: &'a dyn RenderPassNode) {
        self.passes.push(pass);
    }
    
    /// Validate: every read resource is written by a prior pass or is external.
    #[allow(dead_code)]
    pub fn validate(&self, externals: &[ResourceId]) -> Result<(), String> {
        let mut available: HashSet<ResourceId> = externals.iter().copied().collect();
        for pass in &self.passes {
            let decl = pass.declaration();
            for &read in decl.reads {
                if !available.contains(&read) {
                    return Err(format!(
                        "Pass '{}' reads {:?} which is not yet available",
                        decl.name, read,
                    ));
                }
            }
            for &write in decl.writes {
                available.insert(write);
            }
        }
        Ok(())
    }
    
    /// Execute all passes in order, returning the draws they issued.
    pub fn execute(&self, encoder: &mut wgpu::CommandEncoder, resources: &ResourceMap) -> u32 {
        let mut draws = 0;
        for pass in &self.passes {
            draws += pass.record(encoder, resources);
        }
        draws
    }
}