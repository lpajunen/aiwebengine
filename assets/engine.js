/*
 * aiwebengine account menu
 *
 * Defines <aw-account-menu>: a person button in the top right of a page only a
 * signed-in person sees. It asks /auth/status who that is and offers the
 * account page and sign-out. Elements nested inside it (links or buttons) are
 * the page's own items and are listed above those. Published at
 * /engine/engine.js; see docs/ENGINE_STYLES.md.
 *
 * Signed out, it shows a link to sign in instead. It styles itself with the
 * aw- classes in /engine/engine.css, which the page must also carry.
 */
(() => {
  if (customElements.get("aw-account-menu")) return;

  const SVG = "http://www.w3.org/2000/svg";

  function icon() {
    const svg = document.createElementNS(SVG, "svg");
    svg.setAttribute("viewBox", "0 0 24 24");
    svg.setAttribute("width", "18");
    svg.setAttribute("height", "18");
    svg.setAttribute("fill", "none");
    svg.setAttribute("stroke", "currentColor");
    svg.setAttribute("stroke-width", "2");
    svg.setAttribute("stroke-linecap", "round");
    svg.setAttribute("aria-hidden", "true");
    const head = document.createElementNS(SVG, "circle");
    head.setAttribute("cx", "12");
    head.setAttribute("cy", "8");
    head.setAttribute("r", "4");
    const body = document.createElementNS(SVG, "path");
    body.setAttribute("d", "M4 21c0-4.4 3.6-8 8-8s8 3.6 8 8");
    svg.append(head, body);
    return svg;
  }

  function link(text, href) {
    const a = document.createElement("a");
    a.className = "aw-menu-item";
    a.href = href;
    a.textContent = text;
    return a;
  }

  class AccountMenu extends HTMLElement {
    connectedCallback() {
      // An element in the page's markup is connected before its children are
      // parsed, and the children are the page's own menu items.
      if (document.readyState === "loading") {
        document.addEventListener("DOMContentLoaded", () => this.start(), {
          once: true,
        });
      } else {
        this.start();
      }
    }

    start() {
      if (this.ready) return;
      this.ready = true;
      this.extras = Array.from(this.children);
      this.extras.forEach((node) => node.remove());
      this.load();
    }

    async load() {
      let status = null;
      try {
        const response = await fetch("/auth/status", {
          credentials: "same-origin",
          headers: { Accept: "application/json" },
        });
        status = await response.json();
      } catch {
        // Fall through to the signed-out form: a menu that cannot say who you
        // are is better as a way to sign in than as nothing.
      }
      if (status && status.success) this.signedIn(status);
      else this.signedOut();
    }

    signedOut() {
      this.replaceChildren(
        link(
          "Sign in",
          "/auth/login?redirect=" +
            encodeURIComponent(location.pathname + location.search),
        ),
      );
      this.firstChild.classList.add("aw-menu-signin");
    }

    signedIn(status) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "aw-menu-button";
      button.setAttribute("aria-haspopup", "true");
      button.setAttribute("aria-expanded", "false");
      button.setAttribute("aria-label", "Account menu");
      button.title = status.label || "Account";
      button.append(icon());

      const panel = document.createElement("div");
      panel.className = "aw-menu-panel";
      panel.hidden = true;

      const who = document.createElement("div");
      who.className = "aw-menu-who";
      who.textContent = status.label || status.user_id || "";
      panel.append(who);

      for (const node of this.extras) {
        node.classList.add("aw-menu-item");
        panel.append(node);
      }
      if (this.extras.length) {
        const rule = document.createElement("hr");
        rule.className = "aw-menu-rule";
        panel.append(rule);
      }
      panel.append(
        link("Your account", "/auth/account"),
        link(
          "Sign out",
          "/auth/logout?redirect=" + encodeURIComponent(location.pathname),
        ),
      );

      const close = () => {
        panel.hidden = true;
        button.setAttribute("aria-expanded", "false");
      };
      button.addEventListener("click", () => {
        panel.hidden = !panel.hidden;
        button.setAttribute("aria-expanded", String(!panel.hidden));
      });
      panel.addEventListener("click", (event) => {
        if (event.target.closest(".aw-menu-item")) close();
      });
      document.addEventListener("click", (event) => {
        if (!this.contains(event.target)) close();
      });
      document.addEventListener("keydown", (event) => {
        if (event.key === "Escape" && !panel.hidden) {
          close();
          button.focus();
        }
      });

      this.replaceChildren(button, panel);
    }
  }

  customElements.define("aw-account-menu", AccountMenu);
})();
