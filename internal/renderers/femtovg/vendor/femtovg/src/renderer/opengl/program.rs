use std::rc::Rc;

use glow::HasContext;

use crate::{renderer::ShaderType, ErrorKind};

const GLSL_VERSION: &str = "#version 100";

pub struct Shader {
    context: Rc<glow::Context>,
    id: <glow::Context as glow::HasContext>::Shader,
}

impl Shader {
    pub fn new(context: &Rc<glow::Context>, src: &str, kind: u32) -> Result<Self, ErrorKind> {
        let id = unsafe { context.create_shader(kind).unwrap() };

        // Compile
        unsafe {
            context.shader_source(id, src);
            context.compile_shader(id);
        }

        // Validate

        let success = unsafe { context.get_shader_compile_status(id) };
        if !success {
            let error = unsafe { context.get_shader_info_log(id) };

            let name = match kind {
                glow::VERTEX_SHADER => "Vertex stage",
                glow::FRAGMENT_SHADER => "Fragment stage",
                _ => "Shader stage",
            };

            return Err(ErrorKind::ShaderCompileError(format!("{name}: {error}")));
        }

        Ok(Self {
            context: context.clone(),
            id,
        })
    }

    pub fn id(&self) -> <glow::Context as glow::HasContext>::Shader {
        self.id
    }
}

impl Drop for Shader {
    fn drop(&mut self) {
        unsafe {
            self.context.delete_shader(self.id);
        }
    }
}

#[derive(Debug)]
pub struct Program {
    context: Rc<glow::Context>,
    id: <glow::Context as glow::HasContext>::Program,
}

impl Program {
    pub fn new(context: &Rc<glow::Context>, shaders: &[Shader], attrib_locations: &[&str]) -> Result<Self, ErrorKind> {
        let program = Self {
            context: context.clone(),
            id: unsafe { context.create_program().unwrap() },
        };

        // Attach stages
        for shader in shaders {
            unsafe {
                context.attach_shader(program.id, shader.id());
            }
        }

        for (i, loc) in attrib_locations.iter().enumerate() {
            unsafe {
                context.bind_attrib_location(program.id, i as u32, loc);
            }
        }

        unsafe {
            context.link_program(program.id);
        }

        // Check for error

        let success = unsafe { context.get_program_link_status(program.id) };

        if !success {
            let error = unsafe { context.get_program_info_log(program.id) };

            return Err(ErrorKind::ShaderLinkError(error));
        }

        // Detach stages
        for shader in shaders {
            unsafe {
                context.detach_shader(program.id, shader.id());
            }
        }

        Ok(program)
    }

    pub(crate) fn bind(&self) {
        unsafe {
            self.context.use_program(Some(self.id));
        }
    }

    pub(crate) fn unbind(&self) {
        unsafe {
            self.context.use_program(None);
        }
    }

    fn uniform_location(&self, name: &str) -> Option<<glow::Context as glow::HasContext>::UniformLocation> {
        unsafe { self.context.get_uniform_location(self.id, name) }
    }
}

impl Drop for Program {
    fn drop(&mut self) {
        unsafe {
            self.context.delete_program(self.id);
        }
    }
}

#[derive(Debug)]
pub struct MainProgram {
    context: Rc<glow::Context>,
    program: Program,
    loc_viewsize: <glow::Context as glow::HasContext>::UniformLocation,
    loc_tex: Option<<glow::Context as glow::HasContext>::UniformLocation>,
    loc_glyphtex: Option<<glow::Context as glow::HasContext>::UniformLocation>,
    loc_cliptex: Option<<glow::Context as glow::HasContext>::UniformLocation>,
    loc_frag: Option<<glow::Context as glow::HasContext>::UniformLocation>,
    startup_trace: bool,
    shader_type: u8,
    with_glyph_texture: bool,
    used: std::cell::Cell<bool>,
}

// PoC: retain every shader variant, but optionally compile it on first access.
// The normal eager path still reports compilation errors from renderer creation.
#[derive(Debug)]
pub struct ProgramSlot {
    program: std::cell::OnceCell<MainProgram>,
    context: Rc<glow::Context>,
    antialias: bool,
    shader_type: ShaderType,
    with_glyph_texture: bool,
}

impl ProgramSlot {
    pub fn new(
        context: &Rc<glow::Context>,
        antialias: bool,
        shader_type: ShaderType,
        with_glyph_texture: bool,
        lazy: bool,
    ) -> Result<Self, ErrorKind> {
        let program = std::cell::OnceCell::new();
        if !lazy {
            let _ = program.set(MainProgram::new(context, antialias, shader_type, with_glyph_texture)?);
        }
        Ok(Self {
            program,
            context: context.clone(),
            antialias,
            shader_type,
            with_glyph_texture,
        })
    }

    pub fn get(&self) -> &MainProgram {
        self.program.get_or_init(|| {
            MainProgram::new(&self.context, self.antialias, self.shader_type, self.with_glyph_texture)
                .expect("PoC: shader compilation failed on first use")
        })
    }
}

impl MainProgram {
    pub(crate) fn new(
        context: &Rc<glow::Context>,
        antialias: bool,
        shader_type: ShaderType,
        with_glyph_texture: bool,
    ) -> Result<Self, ErrorKind> {
        Self::build(context, antialias, shader_type, with_glyph_texture, (false, false))
    }

    /// The program of a draw under a clip shape, a coverage mask or both -
    /// `clips`, in that order: no other program evaluates one.
    pub(crate) fn with_clip(
        context: &Rc<glow::Context>,
        antialias: bool,
        shader_type: ShaderType,
        with_glyph_texture: bool,
        clips: (bool, bool),
    ) -> Result<Self, ErrorKind> {
        Self::build(context, antialias, shader_type, with_glyph_texture, clips)
    }

    fn build(
        context: &Rc<glow::Context>,
        antialias: bool,
        shader_type: ShaderType,
        with_glyph_texture: bool,
        (with_clip_shape, with_clip_mask): (bool, bool),
    ) -> Result<Self, ErrorKind> {
        let startup_trace = std::env::var("SLINT_TRACE_RENDERER_STARTUP").as_deref() == Ok("1");
        let started = std::time::Instant::now();
        let shader_defs = if antialias { "#define EDGE_AA 1" } else { "" };
        let select_shader_type = format!(
            "#define SELECT_SHADER {}\n{}\n{}\n{}",
            shader_type.to_u8(),
            if with_glyph_texture {
                "#define ENABLE_GLYPH_TEXTURE"
            } else {
                ""
            },
            if with_clip_shape { "#define CLIP_SHAPE" } else { "" },
            if with_clip_mask { "#define CLIP_MASK" } else { "" }
        );
        let vert_shader_src = format!("{}\n{}\n{}", GLSL_VERSION, shader_defs, include_str!("main-vs.glsl"));
        let frag_shader_src = format!(
            "{}\n{}\n{}\n{}\n{}",
            GLSL_VERSION,
            shader_defs,
            select_shader_type,
            include_str!("main-fs.glsl"),
            include_str!("filters.glsl")
        );

        let source_ready = std::time::Instant::now();
        let vert_shader = Shader::new(context, &vert_shader_src, glow::VERTEX_SHADER)?;
        let vertex_ready = std::time::Instant::now();
        let frag_shader = Shader::new(context, &frag_shader_src, glow::FRAGMENT_SHADER)?;
        let fragment_ready = std::time::Instant::now();

        let program = Program::new(context, &[vert_shader, frag_shader], &["vertex", "tcoord"])?;
        let linked = std::time::Instant::now();

        let loc_viewsize = program.uniform_location("viewSize").unwrap();
        let loc_tex = program.uniform_location("tex");
        let loc_glyphtex = program.uniform_location("glyphtex");
        let loc_cliptex = program.uniform_location("cliptex");
        let loc_frag = program.uniform_location("frag");
        let ready = std::time::Instant::now();
        if startup_trace {
            eprintln!(
                "slint-poc-program {} {} {:.6} {:.6} {:.6} {:.6} {:.6}",
                shader_type.to_u8(),
                with_glyph_texture,
                (vertex_ready - source_ready).as_secs_f64() * 1000.0,
                (fragment_ready - vertex_ready).as_secs_f64() * 1000.0,
                (linked - fragment_ready).as_secs_f64() * 1000.0,
                (ready - linked).as_secs_f64() * 1000.0,
                (ready - started).as_secs_f64() * 1000.0,
            );
        }

        Ok(Self {
            context: context.clone(),
            program,
            loc_viewsize,
            loc_tex,
            loc_glyphtex,
            loc_cliptex,
            loc_frag,
            startup_trace,
            shader_type: shader_type.to_u8(),
            with_glyph_texture,
            used: std::cell::Cell::new(false),
        })
    }

    pub(crate) fn set_tex(&self, tex: i32) {
        unsafe {
            self.context.uniform_1_i32(self.loc_tex.as_ref(), tex);
        }
    }

    pub(crate) fn set_glyphtex(&self, tex: i32) {
        unsafe {
            self.context.uniform_1_i32(self.loc_glyphtex.as_ref(), tex);
        }
    }

    pub(crate) fn set_cliptex(&self, tex: i32) {
        unsafe {
            self.context.uniform_1_i32(self.loc_cliptex.as_ref(), tex);
        }
    }

    pub(crate) fn set_view(&self, view: [f32; 2]) {
        unsafe {
            self.context.uniform_2_f32_slice(Some(&self.loc_viewsize), &view);
        }
    }

    pub(crate) fn set_config(&self, config: &[f32]) {
        unsafe {
            self.context.uniform_4_f32_slice(self.loc_frag.as_ref(), config);
        }
    }

    pub(crate) fn bind(&self) {
        if self.startup_trace && !self.used.replace(true) {
            eprintln!(
                "slint-poc-program-used {} {}",
                self.shader_type, self.with_glyph_texture
            );
        }
        self.program.bind();
    }

    pub(crate) fn unbind(&self) {
        self.program.unbind();
    }
}
