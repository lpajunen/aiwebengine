//! `convert` and JSX.

use super::*;
use rquickjs::{Function, Result as JsResult};

/// Builds `convert` over `__hostConvert`.
pub(super) const CONVERT_PRELUDE: &str = include_str!("../../../assets/convert_prelude.js");

impl SecureGlobalContext {
    /// Setup conversion functions (markdown to HTML, etc.)
    pub(super) fn setup_conversion_functions(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        _script_uri: &str,
    ) -> JsResult<()> {
        // `__hostConvert` answers in the envelope `convert_prelude.js` unwraps.
        // A conversion that fails used to answer `"Error: ..."` as its result,
        // which for `markdown_to_html` is indistinguishable from a document
        // that begins with those words.
        let host = rquickjs::Object::new(ctx.clone())?;

        let markdown_to_html = Function::new(ctx.clone(), move |markdown: String| -> String {
            match crate::conversion::convert_markdown_to_html(&markdown) {
                Ok(html) => host_ok(serde_json::Value::String(html)),
                Err(e) => host_failure("Error", &format!("convert.markdown_to_html: {}", e)),
            }
        })?;

        let render_handlebars_template = Function::new(
            ctx.clone(),
            move |template: String, data: String| -> String {
                match crate::conversion::render_handlebars_template(&template, &data) {
                    Ok(rendered) => host_ok(serde_json::Value::String(rendered)),
                    Err(e) => host_failure(
                        "Error",
                        &format!("convert.render_handlebars_template: {}", e),
                    ),
                }
            },
        )?;

        let btoa = Function::new(ctx.clone(), move |input: String| -> String {
            match crate::conversion::convert_btoa(&input) {
                Ok(encoded) => host_ok(serde_json::Value::String(encoded)),
                Err(e) => host_failure("TypeError", &format!("convert.btoa: {}", e)),
            }
        })?;

        let atob = Function::new(ctx.clone(), move |input: String| -> String {
            match crate::conversion::convert_atob(&input) {
                Ok(decoded) => host_ok(serde_json::Value::String(decoded)),
                Err(e) => host_failure("TypeError", &format!("convert.atob: {}", e)),
            }
        })?;

        host.set("markdown_to_html", markdown_to_html)?;
        host.set("render_handlebars_template", render_handlebars_template)?;
        host.set("btoa", btoa)?;
        host.set("atob", atob)?;
        ctx.globals().set("__hostConvert", host)?;
        crate::bytecode::eval_program(ctx, "engine://convert-prelude", CONVERT_PRELUDE).map_err(
            |e| {
                rquickjs::Error::new_from_js_message(
                    "convert",
                    "prelude",
                    &format!("convert prelude failed to load: {}", e),
                )
            },
        )?;

        Ok(())
    }
}

impl SecureGlobalContext {
    /// Setup JSX factory functions for server-side HTML generation
    pub(super) fn setup_jsx_functions(&self, ctx: &rquickjs::Ctx<'_>) -> JsResult<()> {
        // Define the h() function and Fragment in JavaScript to properly handle variadic arguments
        // This approach is more compatible with how JSX transpilation works
        ctx.eval::<(), _>(
            r#"
            // Helper to mark HTML as safe (already escaped)
            function SafeHTML(html) {
                this.__html = html;
                this.__safe = true;
            }
            SafeHTML.prototype.toString = function() {
                return this.__html;
            };
            SafeHTML.prototype.valueOf = function() {
                return this.__html;
            };
            // Make it JSON-serializable
            SafeHTML.prototype.toJSON = function() {
                return this.__html;
            };
            
            globalThis.h = function(tag, props, ...children) {
                // Handle function components (React-style components)
                if (typeof tag === 'function') {
                    // Merge children into props if they exist
                    const componentProps = props || {};
                    if (children.length > 0) {
                        componentProps.children = children.length === 1 ? children[0] : children;
                    }
                    // Call the component function and return its result
                    return tag(componentProps);
                }
                
                // Handle HTML elements (string tags)
                // Build attributes string from props
                let attrsStr = '';
                if (props && typeof props === 'object' && !Array.isArray(props)) {
                    for (const key in props) {
                        if (key === 'children') continue;
                        
                        // Basic attribute validation (prevent XSS)
                        if (!/^[a-zA-Z][a-zA-Z0-9\-]*$/.test(key)) continue;
                        
                        // Skip dangerous event handlers
                        if (/^on/i.test(key)) continue;
                        
                        const value = props[key];
                        if (typeof value === 'boolean') {
                            if (value) {
                                attrsStr += ' ' + key;
                            }
                        } else {
                            // HTML escape the attribute value
                            const escaped = String(value)
                                .replace(/&/g, '&amp;')
                                .replace(/"/g, '&quot;')
                                .replace(/'/g, '&#x27;')
                                .replace(/</g, '&lt;')
                                .replace(/>/g, '&gt;');
                            attrsStr += ' ' + key + '="' + escaped + '"';
                        }
                    }
                }
                
                // Process children
                const processChildren = (items) => {
                    return items.map(child => {
                        if (child === null || child === undefined) return '';
                        
                        // Check if it's safe HTML (from another h() call)
                        if (child && typeof child === 'object' && child.__safe) {
                            return child.__html;
                        }
                        
                        // Check if it's already a SafeHTML result (happens with component returns)
                        if (child instanceof SafeHTML) {
                            return child.__html;
                        }
                        
                        if (typeof child === 'string') {
                            // HTML escape text content (this is raw text from JSX)
                            return child
                                .replace(/&/g, '&amp;')
                                .replace(/</g, '&lt;')
                                .replace(/>/g, '&gt;')
                                .replace(/"/g, '&quot;')
                                .replace(/'/g, '&#x27;');
                        }
                        if (Array.isArray(child)) {
                            return processChildren(child);
                        }
                        return String(child);
                    }).join('');
                };
                
                const childrenHtml = processChildren(children);
                
                // Self-closing tags
                const selfClosing = ['area', 'base', 'br', 'col', 'embed', 'hr', 'img', 
                    'input', 'link', 'meta', 'param', 'source', 'track', 'wbr'];
                if (selfClosing.includes(tag)) {
                    return new SafeHTML('<' + tag + attrsStr + '/>');
                }
                
                // Regular tags with children - return as SafeHTML to prevent double-escaping
                return new SafeHTML('<' + tag + attrsStr + '>' + childrenHtml + '</' + tag + '>');
            };
            
            globalThis.Fragment = function(props, ...children) {
                // Fragment just returns children without a wrapper
                const processChildren = (items) => {
                    return items.map(child => {
                        if (child === null || child === undefined) return '';
                        
                        // Check if it's safe HTML
                        if (child && typeof child === 'object' && child.__safe) {
                            return child.__html;
                        }
                        if (child instanceof SafeHTML) {
                            return child.__html;
                        }
                        
                        if (typeof child === 'string') {
                            return child
                                .replace(/&/g, '&amp;')
                                .replace(/</g, '&lt;')
                                .replace(/>/g, '&gt;')
                                .replace(/"/g, '&quot;')
                                .replace(/'/g, '&#x27;');
                        }
                        if (Array.isArray(child)) {
                            return processChildren(child);
                        }
                        return String(child);
                    }).join('');
                };
                return new SafeHTML(processChildren(children));
            };
            "#,
        )?;

        Ok(())
    }
}
