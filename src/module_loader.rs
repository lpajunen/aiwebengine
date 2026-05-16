use crate::repository;
use crate::transpiler;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path};

const MAX_MODULE_SPECIFIER_LENGTH: usize = 255;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleLoaderError {
    InvalidSpecifier(String),
    UnsupportedImport(String),
    Transpilation(String),
}

impl std::fmt::Display for ModuleLoaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModuleLoaderError::InvalidSpecifier(message)
            | ModuleLoaderError::UnsupportedImport(message)
            | ModuleLoaderError::Transpilation(message) => write!(f, "{}", message),
        }
    }
}

impl std::error::Error for ModuleLoaderError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedExecutable {
    pub code: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleSource {
    pub logical_path: String,
    pub content: String,
    pub mimetype: String,
}

pub fn prepare_executable_program(
    script_uri: &str,
    root_content: &str,
) -> Result<PreparedExecutable, ModuleLoaderError> {
    if contains_dynamic_import(root_content) {
        return Err(ModuleLoaderError::UnsupportedImport(
            "Dynamic import() is not supported for asset-backed modules".to_string(),
        ));
    }

    let root_path = root_module_path(script_uri)?;
    let mut linker = ModuleLinker::new(script_uri);
    let root_code = linker.compile_root_module(&root_path, root_content)?;

    if linker.module_order.is_empty() {
        return Ok(PreparedExecutable { code: root_code });
    }

    let mut bundled = String::from("const __asset_module_factories__ = Object.create(null);\n");
    bundled.push_str("const __asset_module_cache__ = Object.create(null);\n");
    bundled.push_str(
        "function __asset_module_define__(path, factory) { __asset_module_factories__[path] = factory; }\n",
    );
    bundled.push_str("function __asset_module_require__(path) {\n");
    bundled.push_str(
        "  if (__asset_module_cache__[path]) { return __asset_module_cache__[path].exports; }\n",
    );
    bundled.push_str("  const factory = __asset_module_factories__[path];\n");
    bundled.push_str("  if (!factory) { throw new Error(`Unknown asset module: ${path}`); }\n");
    bundled.push_str("  const module = { exports: {} };\n");
    bundled.push_str("  __asset_module_cache__[path] = module;\n");
    bundled.push_str("  factory(module.exports, module);\n");
    bundled.push_str("  return module.exports;\n");
    bundled.push_str("}\n");

    for module_path in &linker.module_order {
        let module_code = linker.compiled_modules.get(module_path).ok_or_else(|| {
            ModuleLoaderError::InvalidSpecifier(format!(
                "Compiled module '{}' was missing from the module cache",
                module_path
            ))
        })?;
        bundled.push_str(&format!(
            "__asset_module_define__({:?}, function(exports, module) {{\n{}\n}});\n",
            module_path, module_code
        ));
    }

    bundled.push_str(&root_code);

    let code = transpiler::transpile_if_needed(script_uri, &bundled).map_err(|error| {
        ModuleLoaderError::Transpilation(format!(
            "Failed transpiling bundled asset-backed program for '{}': {}",
            script_uri, error
        ))
    })?;

    Ok(PreparedExecutable { code })
}

struct ModuleLinker<'a> {
    root_script_uri: &'a str,
    compiled_modules: HashMap<String, String>,
    module_order: Vec<String>,
    visiting: HashSet<String>,
}

impl<'a> ModuleLinker<'a> {
    fn new(root_script_uri: &'a str) -> Self {
        Self {
            root_script_uri,
            compiled_modules: HashMap::new(),
            module_order: Vec::new(),
            visiting: HashSet::new(),
        }
    }

    fn compile_root_module(
        &mut self,
        root_path: &str,
        root_content: &str,
    ) -> Result<String, ModuleLoaderError> {
        let transformed = transform_module_source(root_content, root_path, true)?;
        self.resolve_dependencies(root_path, &transformed)?;

        transpiler::transpile_if_needed(root_path, &transformed.code).map_err(|error| {
            ModuleLoaderError::Transpilation(format!(
                "Failed transpiling root module '{}': {}",
                root_path, error
            ))
        })
    }

    fn compile_dependency_module(
        &mut self,
        module_path: &str,
        module_source: &ModuleSource,
    ) -> Result<(), ModuleLoaderError> {
        if self.compiled_modules.contains_key(module_path) {
            return Ok(());
        }

        if !self.visiting.insert(module_path.to_string()) {
            return Err(ModuleLoaderError::UnsupportedImport(format!(
                "Circular asset-backed module import detected at '{}'",
                module_path
            )));
        }

        let transformed = transform_module_source(&module_source.content, module_path, false)?;
        self.resolve_dependencies(module_path, &transformed)?;

        let compiled = if module_path.ends_with(".json") {
            transform_json_module(&module_source.content, module_path)?
        } else {
            let transpiled = transpiler::transpile_if_needed(module_path, &transformed.code)
                .map_err(|error| {
                    ModuleLoaderError::Transpilation(format!(
                        "Failed transpiling asset module '{}': {}\nTransformed module source:\n{}",
                        module_path, error, transformed.code
                    ))
                })?;
            if transformed.export_footer.is_empty() {
                transpiled
            } else {
                format!("{}\n{}", transpiled, transformed.export_footer.join("\n"))
            }
        };

        self.visiting.remove(module_path);
        self.compiled_modules
            .insert(module_path.to_string(), compiled);
        self.module_order.push(module_path.to_string());
        Ok(())
    }

    fn resolve_dependencies(
        &mut self,
        importer_path: &str,
        transformed: &TransformedModule,
    ) -> Result<(), ModuleLoaderError> {
        for dependency in &transformed.dependencies {
            if self.compiled_modules.contains_key(dependency) {
                continue;
            }

            let asset_source = load_owned_asset_module_by_path(
                self.root_script_uri,
                dependency,
                dependency,
                importer_path,
            )?;
            self.compile_dependency_module(dependency, &asset_source)?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone)]
struct TransformedModule {
    code: String,
    dependencies: Vec<String>,
    export_footer: Vec<String>,
}

struct StaticImportPatterns {
    default_and_named: Regex,
    named: Regex,
    default: Regex,
    side_effect: Regex,
}

impl StaticImportPatterns {
    fn new() -> Self {
        Self {
            default_and_named: Regex::new(
                r#"^\s*import\s+([A-Za-z_$][\w$]*)\s*,\s*\{([^}]*)\}\s+from\s+['\"]([^'\"]+)['\"]\s*;?\s*$"#,
            )
            .expect("default and named import regex should compile"),
            named: Regex::new(r#"^\s*import\s+\{([^}]*)\}\s+from\s+['\"]([^'\"]+)['\"]\s*;?\s*$"#)
                .expect("named import regex should compile"),
            default: Regex::new(r#"^\s*import\s+([A-Za-z_$][\w$]*)\s+from\s+['\"]([^'\"]+)['\"]\s*;?\s*$"#)
                .expect("default import regex should compile"),
            side_effect: Regex::new(r#"^\s*import\s+['\"]([^'\"]+)['\"]\s*;?\s*$"#)
                .expect("side effect import regex should compile"),
        }
    }
}

fn transform_module_source(
    source: &str,
    importer_path: &str,
    is_root: bool,
) -> Result<TransformedModule, ModuleLoaderError> {
    if contains_dynamic_import(source) {
        return Err(ModuleLoaderError::UnsupportedImport(
            "Dynamic import() is not supported for asset-backed modules".to_string(),
        ));
    }

    let import_patterns = StaticImportPatterns::new();

    let mut generated_imports = Vec::new();
    let mut kept_lines = Vec::new();
    let mut dependencies = Vec::new();
    let mut pending_import: Option<String> = None;

    for line in source.lines() {
        if let Some(statement) = pending_import.as_mut() {
            statement.push('\n');
            statement.push_str(line);

            if try_rewrite_import_statement(
                statement,
                importer_path,
                &import_patterns,
                &mut generated_imports,
                &mut dependencies,
            )? {
                pending_import = None;
            } else if line.trim_end().ends_with(';') {
                kept_lines.extend(statement.lines().map(str::to_string));
                pending_import = None;
            }
            continue;
        }

        if try_rewrite_import_statement(
            line,
            importer_path,
            &import_patterns,
            &mut generated_imports,
            &mut dependencies,
        )? {
            continue;
        }

        if line.trim_start().starts_with("import ") {
            pending_import = Some(line.to_string());
            continue;
        }

        kept_lines.push(line.to_string());
    }

    if let Some(statement) = pending_import
        && !try_rewrite_import_statement(
            &statement,
            importer_path,
            &import_patterns,
            &mut generated_imports,
            &mut dependencies,
        )?
    {
        kept_lines.extend(statement.lines().map(str::to_string));
    }

    let mut body = kept_lines.join("\n");
    if is_root && contains_static_export(&body) {
        return Err(ModuleLoaderError::UnsupportedImport(
            "Root scripts cannot use export syntax; define global handlers instead".to_string(),
        ));
    }

    let mut export_footer = Vec::new();
    if !is_root {
        body = rewrite_exports(&body, &mut export_footer)?;
    }

    let code = if generated_imports.is_empty() {
        body
    } else if body.trim().is_empty() {
        generated_imports.join("\n")
    } else {
        format!("{}\n{}", generated_imports.join("\n"), body)
    };

    Ok(TransformedModule {
        code,
        dependencies,
        export_footer,
    })
}

fn try_rewrite_import_statement(
    statement: &str,
    importer_path: &str,
    import_patterns: &StaticImportPatterns,
    generated_imports: &mut Vec<String>,
    dependencies: &mut Vec<String>,
) -> Result<bool, ModuleLoaderError> {
    if let Some(captures) = import_patterns.default_and_named.captures(statement) {
        let default_binding = captures.get(1).expect("default binding capture").as_str();
        let named_bindings = captures.get(2).expect("named binding capture").as_str();
        let specifier = captures.get(3).expect("specifier capture").as_str();
        let resolved = normalize_asset_module_specifier(importer_path, specifier)?;
        dependencies.push(resolved.clone());
        let temp_binding = format!("__asset_module_{}", generated_imports.len());
        generated_imports.push(format!(
            "const {} = __asset_module_require__({:?});",
            temp_binding, resolved
        ));
        generated_imports.push(format!(
            "const {} = {}.default;",
            default_binding, temp_binding
        ));
        generated_imports.push(render_named_binding_assignment(
            &temp_binding,
            named_bindings,
        )?);
        return Ok(true);
    }

    if let Some(captures) = import_patterns.named.captures(statement) {
        let named_bindings = captures.get(1).expect("named binding capture").as_str();
        let specifier = captures.get(2).expect("specifier capture").as_str();
        let resolved = normalize_asset_module_specifier(importer_path, specifier)?;
        dependencies.push(resolved.clone());
        generated_imports.push(render_named_binding_assignment(
            &format!("__asset_module_require__({:?})", resolved),
            named_bindings,
        )?);
        return Ok(true);
    }

    if let Some(captures) = import_patterns.default.captures(statement) {
        let default_binding = captures.get(1).expect("default binding capture").as_str();
        let specifier = captures.get(2).expect("specifier capture").as_str();
        let resolved = normalize_asset_module_specifier(importer_path, specifier)?;
        dependencies.push(resolved.clone());
        generated_imports.push(format!(
            "const {} = __asset_module_require__({:?}).default;",
            default_binding, resolved
        ));
        return Ok(true);
    }

    if let Some(captures) = import_patterns.side_effect.captures(statement) {
        let specifier = captures.get(1).expect("specifier capture").as_str();
        let resolved = normalize_asset_module_specifier(importer_path, specifier)?;
        dependencies.push(resolved.clone());
        generated_imports.push(format!("__asset_module_require__({:?});", resolved));
        return Ok(true);
    }

    Ok(false)
}

fn render_named_binding_assignment(
    source_expr: &str,
    named_bindings: &str,
) -> Result<String, ModuleLoaderError> {
    let mut bindings = Vec::new();
    for binding in named_bindings.split(',') {
        let trimmed = binding.trim();
        if trimmed.is_empty() {
            continue;
        }

        let mut parts = trimmed.split(" as ").map(str::trim);
        let exported = parts.next().unwrap_or_default();
        let local = parts.next().unwrap_or(exported);
        if exported.is_empty() || local.is_empty() {
            return Err(ModuleLoaderError::InvalidSpecifier(format!(
                "Unsupported named import binding '{}'",
                trimmed
            )));
        }
        if exported == local {
            bindings.push(exported.to_string());
        } else {
            bindings.push(format!("{}: {}", exported, local));
        }
    }

    if bindings.is_empty() {
        return Err(ModuleLoaderError::InvalidSpecifier(
            "Named import list cannot be empty".to_string(),
        ));
    }

    Ok(format!(
        "const {{ {} }} = {};",
        bindings.join(", "),
        source_expr
    ))
}

fn rewrite_exports(
    source: &str,
    export_footer: &mut Vec<String>,
) -> Result<String, ModuleLoaderError> {
    let mut rewritten = source.to_string();

    if rewritten.contains("export default") {
        return Err(ModuleLoaderError::UnsupportedImport(
            "Default exports are only supported for JSON asset modules in v1".to_string(),
        ));
    }

    let export_function = Regex::new(r"(?m)^(\s*)export\s+function\s+([A-Za-z_$][\w$]*)")
        .expect("export function regex should compile");
    for capture in export_function.captures_iter(source) {
        export_footer.push(format!(
            "exports.{0} = {0};",
            capture.get(2).expect("function export name").as_str()
        ));
    }
    rewritten = export_function
        .replace_all(&rewritten, "${1}function ${2}")
        .to_string();

    let export_binding =
        Regex::new(r"(?m)^(\s*)export\s+(const|let|var|class)\s+([A-Za-z_$][\w$]*)")
            .expect("export binding regex should compile");
    for capture in export_binding.captures_iter(source) {
        export_footer.push(format!(
            "exports.{0} = {0};",
            capture.get(3).expect("binding export name").as_str()
        ));
    }
    rewritten = export_binding
        .replace_all(&rewritten, "${1}${2} ${3}")
        .to_string();

    let export_type =
        Regex::new(r"(?m)^(\s*)export\s+type\s+").expect("export type regex should compile");
    rewritten = export_type.replace_all(&rewritten, "${1}type ").to_string();

    let export_interface = Regex::new(r"(?m)^(\s*)export\s+interface\s+")
        .expect("export interface regex should compile");
    rewritten = export_interface
        .replace_all(&rewritten, "${1}interface ")
        .to_string();

    let export_list = Regex::new(r"(?m)^\s*export\s*\{([^}]*)\}\s*;?\s*$")
        .expect("export list regex should compile");
    for capture in export_list.captures_iter(source) {
        let list = capture.get(1).expect("export list capture").as_str();
        for binding in list.split(',') {
            let trimmed = binding.trim();
            if trimmed.is_empty() {
                continue;
            }
            let mut parts = trimmed.split(" as ").map(str::trim);
            let local = parts.next().unwrap_or_default();
            let exported = parts.next().unwrap_or(local);
            export_footer.push(format!("exports.{exported} = {local};"));
        }
    }
    rewritten = export_list.replace_all(&rewritten, "").to_string();

    Ok(rewritten)
}

fn transform_json_module(content: &str, module_path: &str) -> Result<String, ModuleLoaderError> {
    let parsed: serde_json::Value = serde_json::from_str(content).map_err(|error| {
        ModuleLoaderError::InvalidSpecifier(format!(
            "JSON module '{}' is invalid: {}",
            module_path, error
        ))
    })?;

    Ok(format!("exports.default = {};", parsed))
}

pub fn root_module_path(script_uri: &str) -> Result<String, ModuleLoaderError> {
    let file_name = script_uri
        .rsplit('/')
        .next()
        .filter(|segment| !segment.is_empty())
        .ok_or_else(|| {
            ModuleLoaderError::InvalidSpecifier(format!(
                "Script URI '{}' does not contain a module file name",
                script_uri
            ))
        })?;

    Ok(file_name.to_string())
}

pub fn load_owned_asset_module(
    root_script_uri: &str,
    importer_path: &str,
    specifier: &str,
) -> Result<ModuleSource, ModuleLoaderError> {
    let logical_path = normalize_asset_module_specifier(importer_path, specifier)?;
    load_owned_asset_module_by_path(root_script_uri, &logical_path, specifier, importer_path)
}

fn load_owned_asset_module_by_path(
    root_script_uri: &str,
    logical_path: &str,
    original_specifier: &str,
    importer_path: &str,
) -> Result<ModuleSource, ModuleLoaderError> {
    let asset = repository::fetch_asset(root_script_uri, logical_path).ok_or_else(|| {
        ModuleLoaderError::InvalidSpecifier(format!(
            "Module '{}' imported from '{}' was not found in assets for '{}'",
            original_specifier, importer_path, root_script_uri
        ))
    })?;

    if !is_supported_module_asset(logical_path, &asset.mimetype) {
        return Err(ModuleLoaderError::InvalidSpecifier(format!(
            "Module '{}' has unsupported asset type '{}'",
            logical_path, asset.mimetype
        )));
    }

    let content = String::from_utf8(asset.content).map_err(|_| {
        ModuleLoaderError::InvalidSpecifier(format!(
            "Module '{}' must be valid UTF-8 text content",
            logical_path
        ))
    })?;

    Ok(ModuleSource {
        logical_path: logical_path.to_string(),
        content,
        mimetype: asset.mimetype,
    })
}

pub fn normalize_asset_module_specifier(
    importer_path: &str,
    specifier: &str,
) -> Result<String, ModuleLoaderError> {
    if specifier.is_empty() {
        return Err(ModuleLoaderError::InvalidSpecifier(
            "Module specifier cannot be empty".to_string(),
        ));
    }

    if specifier.len() > MAX_MODULE_SPECIFIER_LENGTH {
        return Err(ModuleLoaderError::InvalidSpecifier(format!(
            "Module specifier too long (max {} characters)",
            MAX_MODULE_SPECIFIER_LENGTH
        )));
    }

    if specifier.contains('\\') {
        return Err(ModuleLoaderError::InvalidSpecifier(
            "Module specifier cannot contain backslashes".to_string(),
        ));
    }

    if specifier.starts_with('/') {
        return Err(ModuleLoaderError::InvalidSpecifier(
            "Module specifier must be relative".to_string(),
        ));
    }

    if !specifier.starts_with("./") && !specifier.starts_with("../") {
        if is_root_asset_specifier(specifier) {
            return normalize_relative_path(Path::new(specifier));
        }

        return Err(ModuleLoaderError::InvalidSpecifier(
            "Module specifier must start with './' or '../', or use an asset-root path like 'server/module.ts'"
                .to_string(),
        ));
    }

    let importer_dir = Path::new(importer_path)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    let candidate = importer_dir.join(specifier);
    let normalized = normalize_relative_path(&candidate)?;

    if normalized.is_empty() {
        return Err(ModuleLoaderError::InvalidSpecifier(
            "Module specifier resolved to an empty path".to_string(),
        ));
    }

    Ok(normalized)
}

fn is_root_asset_specifier(specifier: &str) -> bool {
    specifier.contains('/')
        && !specifier.starts_with('@')
        && !specifier.contains(":")
        && !specifier.starts_with("//")
}

fn normalize_relative_path(path: &Path) -> Result<String, ModuleLoaderError> {
    let mut normalized_parts = Vec::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => normalized_parts.push(part.to_string_lossy().to_string()),
            Component::ParentDir => {
                if normalized_parts.pop().is_none() {
                    return Err(ModuleLoaderError::InvalidSpecifier(
                        "Module specifier escapes the owning script assets".to_string(),
                    ));
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(ModuleLoaderError::InvalidSpecifier(
                    "Module specifier must stay within relative asset paths".to_string(),
                ));
            }
        }
    }

    Ok(normalized_parts.join("/"))
}

fn is_supported_module_asset(logical_path: &str, mimetype: &str) -> bool {
    logical_path.ends_with(".js")
        || logical_path.ends_with(".ts")
        || logical_path.ends_with(".jsx")
        || logical_path.ends_with(".tsx")
        || logical_path.ends_with(".json")
        || matches!(
            mimetype,
            "text/javascript" | "application/javascript" | "text/plain" | "application/json"
        )
}

fn contains_dynamic_import(source: &str) -> bool {
    source.contains("import(")
}

fn contains_static_export(source: &str) -> bool {
    source
        .lines()
        .any(|line| line.trim_start().starts_with("export "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_relative_specifier_in_same_folder() {
        let normalized = normalize_asset_module_specifier("server/main.ts", "./utils.ts")
            .expect("specifier should normalize");
        assert_eq!(normalized, "server/utils.ts");
    }

    #[test]
    fn root_module_path_uses_script_basename() {
        let logical_root = root_module_path("https://example.com/apps/main.ts")
            .expect("script uri basename should be extracted");
        assert_eq!(logical_root, "main.ts");
    }

    #[test]
    fn normalize_relative_specifier_in_parent_folder() {
        let normalized =
            normalize_asset_module_specifier("server/routes/index.ts", "../shared/format.ts")
                .expect("specifier should normalize");
        assert_eq!(normalized, "server/shared/format.ts");
    }

    #[test]
    fn normalize_asset_root_path_specifier() {
        let normalized = normalize_asset_module_specifier("server/main.ts", "shared/format.ts")
            .expect("asset-root specifier should normalize");
        assert_eq!(normalized, "shared/format.ts");
    }

    #[test]
    fn normalize_root_asset_specifier() {
        let normalized = normalize_asset_module_specifier("main.ts", "server/world-domain.ts")
            .expect("root asset specifier should normalize");
        assert_eq!(normalized, "server/world-domain.ts");
    }

    #[test]
    fn reject_url_style_specifier() {
        let error = normalize_asset_module_specifier(
            "server/main.ts",
            "https://example.com/shared/format.ts",
        )
        .expect_err("url specifier should be rejected");
        assert_eq!(
            error,
            ModuleLoaderError::InvalidSpecifier(
                "Module specifier must start with './' or '../', or use an asset-root path like 'server/module.ts'"
                    .to_string(),
            )
        );
    }

    #[test]
    fn reject_package_name_specifier() {
        let error = normalize_asset_module_specifier("server/main.ts", "react")
            .expect_err("package-name specifier should be rejected");
        assert_eq!(
            error,
            ModuleLoaderError::InvalidSpecifier(
                "Module specifier must start with './' or '../', or use an asset-root path like 'server/module.ts'"
                    .to_string(),
            )
        );
    }

    #[test]
    fn reject_absolute_specifier() {
        let error = normalize_asset_module_specifier("server/main.ts", "/shared/format.ts")
            .expect_err("absolute specifier should be rejected");
        assert_eq!(
            error,
            ModuleLoaderError::InvalidSpecifier("Module specifier must be relative".to_string(),)
        );
    }

    #[test]
    fn reject_backslash_specifier() {
        let error = normalize_asset_module_specifier("server/main.ts", ".\\shared\\format.ts")
            .expect_err("backslashes should be rejected");
        assert_eq!(
            error,
            ModuleLoaderError::InvalidSpecifier(
                "Module specifier cannot contain backslashes".to_string(),
            )
        );
    }

    #[test]
    fn reject_path_escape() {
        let error = normalize_asset_module_specifier("server/main.ts", "../../secret.ts")
            .expect_err("path escape should be rejected");
        assert_eq!(
            error,
            ModuleLoaderError::InvalidSpecifier(
                "Module specifier escapes the owning script assets".to_string(),
            )
        );
    }

    #[test]
    fn prepare_executable_program_transpiles_without_imports() {
        let prepared = prepare_executable_program(
            "server/main.ts",
            "const message: string = 'hello';\nconsole.log(message);",
        )
        .expect("script without imports should prepare");

        assert!(prepared.code.contains("message"));
        assert!(!prepared.code.contains(": string"));
    }

    #[test]
    fn transform_module_source_rewrites_multiline_named_imports() {
        let transformed = transform_module_source(
            r#"
import {
  foo,
  bar as baz,
} from "server/world-domain.ts";

function handle() {
  return foo + baz;
}
"#,
            "main.ts",
            true,
        )
        .expect("multiline import should rewrite");

        assert!(transformed.code.contains(
            "const { foo, bar: baz } = __asset_module_require__(\"server/world-domain.ts\");"
        ));
        assert!(transformed.code.contains("function handle()"));
        assert_eq!(
            transformed.dependencies,
            vec!["server/world-domain.ts".to_string()]
        );
    }

    #[test]
    fn transform_module_source_rewrites_type_only_exports() {
        let transformed = transform_module_source(
            r#"
export type WorldType = "forest" | "cave";

export interface WorldTileDef {
  value: number;
}

export const WORLD_TYPE_FOREST: WorldType = "forest";
"#,
            "server/world-domain.ts",
            false,
        )
        .expect("type-only exports should rewrite");

        assert!(!transformed.code.contains("export type"));
        assert!(!transformed.code.contains("export interface"));
        assert!(!transformed.code.contains("export const"));
        assert!(transformed.code.contains("type WorldType"));
        assert!(transformed.code.contains("interface WorldTileDef"));
        assert_eq!(
            transformed.export_footer,
            vec!["exports.WORLD_TYPE_FOREST = WORLD_TYPE_FOREST;".to_string()]
        );
    }

    #[test]
    fn prepare_executable_program_rejects_dynamic_imports() {
        let error = prepare_executable_program("server/main.ts", "const x = import('./x.ts');")
            .expect_err("dynamic imports should be rejected");

        assert_eq!(
            error,
            ModuleLoaderError::UnsupportedImport(
                "Dynamic import() is not supported for asset-backed modules".to_string(),
            )
        );
    }
}
