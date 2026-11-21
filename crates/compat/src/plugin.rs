//! The native-plugin architecture: the [`NativePlugin`] trait
//! and the [`PluginRegistry`] that activates built-ins by name.

use std::collections::BTreeSet;
use std::sync::Mutex;

use crate::CompatCtx;
use crate::CompatError;

/// A statically built-in native plugin emulation.
///
/// A plugin is a named unit (the name scripts pass to `Plugins.link`, e.g.
/// `"menu.dll"`); registration happens at link time and the optional
/// [`Self::poll`] hook runs once per frame.
pub trait NativePlugin {
    /// The name scripts use: `"menu.dll"`, `"windowEx.dll"`, ...
    fn name(&self) -> &str;

    /// Alternate DLL names that resolve to this plugin (for example
    /// `layerExDraw.dll` and `perspective.dll` both map to the `layerEx`
    /// port). Default: none.
    fn aliases(&self) -> &[&str] {
        &[]
    }

    /// Modules this plugin loads when it activates (the pre-registration
    /// dependency, e.g. `emoteplayer.dll` → `motionplayer.dll`). The registry
    /// registers each dependency first and reports it in `Plugins.getList`:
    /// the dependency is a module that was loaded, and the list names every
    /// module that was loaded. Default: none.
    fn dependencies(&self) -> &[&str] {
        &[]
    }

    /// Register the plugin's native classes on the VM.
    ///
    /// # Errors
    ///
    /// Returns [`CompatError::Register`] when a class or a member of the
    /// plugin cannot be registered on the VM.
    fn register(&self, ctx: &mut CompatCtx) -> Result<(), CompatError>;

    /// Optional per-frame hook (async triggers, video overlay, timers).
    ///
    /// # Errors
    ///
    /// Returns [`CompatError::Engine`] when the hook cannot advance its
    /// subsystem; the frame body stops at the first failing poll.
    fn poll(&self, _ctx: &mut CompatCtx) -> Result<(), CompatError> {
        Ok(())
    }
}

/// One entry of the built-in plugin table
/// ([`crate::natives::plugins::BUILTIN_PLUGINS`]).
///
/// `aliases` are additional DLL names that resolve to the same `register`,
/// so the classes are registered once even when a game links several of them.
///
/// The entry *is* the [`NativePlugin`] implementation: adding a plugin to the
/// table is one row rather than a trait impl.
#[derive(Clone, Copy)]
pub struct BuiltinPluginDef {
    /// The canonical DLL name (`"layerExDraw.dll"`).
    pub name: &'static str,
    /// Alternate DLL names resolving to the same `register`.
    pub aliases: &'static [&'static str],
    /// Prerequisite modules, registered first when this plugin activates;
    /// they are recorded as linked modules.
    pub dependencies: &'static [&'static str],
    /// Registers the plugin's native classes when `Plugins.link` activates
    /// it, except for the [`crate::natives::plugins::STARTUP_PLUGINS`] the
    /// host registers once at startup (see [`crate::natives::plugins::register`]).
    pub register: fn(&mut CompatCtx) -> Result<(), CompatError>,
}

impl NativePlugin for BuiltinPluginDef {
    fn name(&self) -> &str {
        self.name
    }

    fn aliases(&self) -> &[&str] {
        self.aliases
    }

    fn dependencies(&self) -> &[&str] {
        self.dependencies
    }

    fn register(&self, ctx: &mut CompatCtx) -> Result<(), CompatError> {
        (self.register)(ctx)
    }
}

/// The result of activating a plugin name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginOutcome {
    /// A built-in emulation was registered.
    Builtin,
    /// The plugin has no emulation: one warning names the plugin and nothing
    /// is registered for it — an absent plugin reads as absent.
    Unported,
}

/// Holds the built-in plugins and activates them by name.
///
/// `link` is idempotent: a name that was already activated is not registered
/// twice (the C++ side rejects duplicate class names). A name with no
/// built-in emulation is warned about and nothing is registered for it: the
/// absent plugin reads as absent.
pub struct PluginRegistry {
    plugins: Vec<Box<dyn NativePlugin>>,
    /// The modules that are linked, sorted by name: what `Plugins.getList`
    /// reports. A name is in it when a link resolved to a built-in, when a
    /// plugin pulled it in as its dependency, or when the startup load put
    /// it on the VM. A name that resolved to nothing is not in it, because
    /// nothing was loaded for it.
    modules: Mutex<BTreeSet<String>>,
    /// Built-ins whose native classes are already on the VM. The ABI cannot
    /// unregister a class, so a later `link` of the same built-in skips
    /// `register` instead of hitting the duplicate-class rejection.
    installed: Mutex<BTreeSet<String>>,
    /// The names a link named and no built-in answered, so the warning names
    /// each of them once.
    warned: Mutex<BTreeSet<String>>,
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginRegistry {
    /// An empty registry (no built-in emulations yet).
    ///
    /// The startup modules are marked installed because
    /// The host's install always puts their surface on the VM before any
    /// script runs, whichever registry the host was built with. They are
    /// marked linked too: the startup load put them on the VM, so the
    /// `Plugins.getList` list names them from the first line.
    pub fn new() -> Self {
        let startup: BTreeSet<String> = crate::natives::plugins::STARTUP_PLUGINS
            .iter()
            .map(|name| normalize_plugin_name(name))
            .collect();
        Self {
            plugins: Vec::new(),
            modules: Mutex::new(startup.clone()),
            installed: Mutex::new(startup),
            warned: Mutex::new(BTreeSet::new()),
        }
    }

    /// The built-in registry assembled at startup.
    ///
    /// Every plugin in the built-in table is added here. The module
    /// available before any script runs (today `xp3filter.dll`) is marked
    /// installed by [`Self::new`]; a later `Plugins.link` of any
    /// other built-in activates it and registers its classes. A name that is
    /// not in the table is warned about, and nothing is registered for it.
    pub fn builtin() -> Self {
        let mut registry = Self::new();
        for def in crate::natives::plugins::BUILTIN_PLUGINS {
            registry.register(Box::new(*def));
        }
        registry
    }

    /// Add a built-in plugin to the registry.
    pub fn register(&mut self, plugin: Box<dyn NativePlugin>) {
        self.plugins.push(plugin);
    }

    /// Look up a built-in plugin by its script-visible name (canonical name
    /// or any alias).
    pub fn plugin(&self, name: &str) -> Option<&dyn NativePlugin> {
        let key = normalize_plugin_name(name);
        self.plugins
            .iter()
            .map(|plugin| plugin.as_ref())
            .find(|plugin| {
                normalize_plugin_name(plugin.name()) == key
                    || plugin
                        .aliases()
                        .iter()
                        .any(|alias| normalize_plugin_name(alias) == key)
            })
    }

    /// Whether `name` has a built-in emulation.
    pub fn is_builtin(&self, name: &str) -> bool {
        self.plugin(name).is_some()
    }

    /// The names of the modules that are linked, lower-cased and sorted by
    /// name: what `Plugins.getList` reports.
    ///
    /// A module is linked when a link resolved to it, when a plugin pulled
    /// it in as its dependency, or when the startup load put it on the VM. A
    /// name that resolved to nothing is not in it, because nothing was
    /// loaded for it.
    pub fn linked(&self) -> Vec<String> {
        self.modules
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .iter()
            .cloned()
            .collect()
    }

    /// Activate a plugin name: register the built-in emulation, or warn when
    /// the name has no emulation.
    ///
    /// An unported name registers nothing: no global under the plugin's stem
    /// and no object that swallows member accesses. Nothing is loaded for it,
    /// so `Plugins.getList` does not name it, and one warning names it.
    ///
    /// # Errors
    ///
    /// Returns [`CompatError::Register`] when the plugin's own registration
    /// fails.
    pub fn activate(&self, name: &str, ctx: &mut CompatCtx) -> Result<PluginOutcome, CompatError> {
        let module = module_name(name);
        if self.is_linked(&module) {
            return Ok(PluginOutcome::Builtin);
        }
        if let Some(plugin) = self.plugin(&module) {
            // Track the canonical name so aliases sharing one `register`
            // (e.g. `layerExDraw.dll` / `perspective.dll`) register once.
            self.ensure_registered(plugin, ctx)?;
            self.mark_linked(&module);
            Ok(PluginOutcome::Builtin)
        } else {
            self.warn_unported(name);
            Ok(PluginOutcome::Unported)
        }
    }

    /// Unload a name.
    ///
    /// Unloading changes nothing: the module stays loaded, its classes stay
    /// on the VM, and `Plugins.getList` keeps naming it. The answer is
    /// `true` whatever the name is.
    pub fn unlink(&self, _name: &str) -> bool {
        true
    }

    /// Run the per-frame hook of every built-in plugin that has been
    /// activated.
    ///
    /// A plugin that no script has linked has no script-visible surface and
    /// owns no per-frame hook, so its `poll` must not run: the `installed`
    /// set is the same one `activate` fills on the first link.
    ///
    /// # Errors
    ///
    /// Returns the first error a plugin's `poll` reports; the remaining
    /// plugins do not run after it.
    pub fn poll(&self, ctx: &mut CompatCtx) -> Result<(), CompatError> {
        let installed = self
            .installed
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        for plugin in &self.plugins {
            if installed.contains(&normalize_plugin_name(plugin.name())) {
                plugin.poll(ctx)?;
            }
        }
        Ok(())
    }

    /// Auto-load the `.tpm` plugins the composition root found, in order.
    ///
    /// The reference `tvpLoadPlugins` scans the exe directory,
    /// `<exe>/system` and `<exe>/plugin` for `*.tpm`, sorts them by file name
    /// and loads each in turn (`reference/cpp/core/plugin/PluginImpl.cpp:148-177`,
    /// called from `reference/cpp/core/environ/Application.cpp:342`). The scan
    /// itself is the composition root's job (compat owns no file-system
    /// access); this receives the ordered file names and links each. A
    /// trailing `.tpm` becomes `.dll` first, matching
    /// `TVPLoadInternalPlugin` (`PluginImpl.cpp:113-133`), and each is logged
    /// with the reference's `(info) Loading <name>` line (`:173-174`).
    ///
    /// Linking goes through the same path as `Plugins.link`: a ported name
    /// activates its built-in emulation, an unported name is warned about and
    /// nothing is loaded for it (a DLL that cannot be loaded creates no
    /// global).
    ///
    /// # Errors
    ///
    /// Returns [`CompatError::Register`] when a named plugin cannot be
    /// registered. The names already loaded stay loaded.
    pub fn autoload<I, S>(&self, names: I, ctx: &mut CompatCtx) -> Result<(), CompatError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for name in names {
            let name = name.as_ref();
            log::info!("(info) Loading {name}");
            self.activate(name, ctx)?;
        }
        Ok(())
    }

    /// Whether the module `module` names is linked.
    fn is_linked(&self, module: &str) -> bool {
        self.modules
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .contains(module)
    }

    /// Record that the module `module` names is linked.
    fn mark_linked(&self, module: &str) {
        self.modules
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(module.to_string());
    }

    /// Warn once that the name `name` has no built-in emulation.
    fn warn_unported(&self, name: &str) {
        let mut warned = self
            .warned
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if warned.insert(module_name(name)) {
            log::warn!("plugin {name:?}: no built-in emulation; the plugin is not available");
        }
    }

    /// Register `plugin` and its dependencies once, and record the modules
    /// as linked.
    ///
    /// The dependencies are registered first, so a plugin's own registration
    /// finds their surface, and each dependency is recorded as a linked
    /// module: the dependency load runs through the same path as the
    /// plugin's own, and `Plugins.getList` names every module that was
    /// loaded. The `installed` set keeps a second load of the same module
    /// (an explicit link or another dependent) from registering its classes
    /// twice.
    fn ensure_registered(
        &self,
        plugin: &dyn NativePlugin,
        ctx: &mut CompatCtx,
    ) -> Result<(), CompatError> {
        let canonical = normalize_plugin_name(plugin.name());
        if self
            .installed
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .contains(&canonical)
        {
            return Ok(());
        }
        for dependency in plugin.dependencies() {
            match self.plugin(dependency) {
                Some(resolved) => {
                    self.ensure_registered(resolved, ctx)?;
                    self.mark_linked(&normalize_plugin_name(dependency));
                }
                None => log::debug!(
                    "plugin {}: dependency {dependency:?} has no built-in emulation",
                    plugin.name()
                ),
            }
        }
        plugin.register(ctx)?;
        self.installed
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(canonical);
        Ok(())
    }
}

/// Normalize a plugin name to its base file name, lowercased, so
/// `C:\game\plugin\extrans.dll`, `extrans.tpm` and `Extrans.DLL` compare
/// equal.
pub fn normalize_plugin_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\', '>']).next().unwrap_or(name);
    base.trim().to_ascii_lowercase()
}

/// The module a name resolves to: the base file name, with a trailing `.tpm`
/// rewritten to `.dll`, lowercased.
///
/// The extension is rewritten before the name is looked up, so `foo.tpm`,
/// `foo.dll` and `FOO.DLL` name one module, and the name is lowercased
/// because that is the key the module table is indexed by.
fn module_name(name: &str) -> String {
    normalize_plugin_name(&tpm_to_dll_name(name))
}

/// Replaces a trailing `.tpm` with `.dll` (case-insensitive), preserving the
/// stem's original casing.
///
/// The reference does the same before loading an internal plugin
/// (`reference/cpp/core/plugin/PluginImpl.cpp:113-133`), so an auto-loaded
/// `foo.tpm` resolves the same built-in as `Plugins.link("foo.dll")`. A name
/// without the extension is returned unchanged.
fn tpm_to_dll_name(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    let Some(stem) = lower.strip_suffix(".tpm") else {
        return name.to_string();
    };
    // `to_ascii_lowercase` preserves byte length, so the stem slice is valid.
    let mut out = name[..stem.len()].to_string();
    out.push_str(".dll");
    out
}
