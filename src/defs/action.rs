#[derive(Debug, Clone)]
#[expect(dead_code, reason = "not yet used")]
pub struct ActionDef {
    pub arguments: Vec<ActionArgumentDef>,
    pub rhai_closure: (),
    pub description: Option<String>,
}

impl ActionDef {
    #[expect(dead_code, reason = "not yet used")]
    pub fn is_shell(&self) -> bool {
        self.arguments
            .iter()
            .any(|arg| matches!(arg, ActionArgumentDef::ShellAttach))
    }
}

#[derive(Debug, Clone, Copy)]
#[expect(dead_code, reason = "not yet used")]
pub enum ActionArgumentDef {
    Runtime,
    ShellAttach,
    OldAppDef,
    AppHistory,
    InstallRequirements,
}
