# Extra Caddy site blocks

Every `*.caddy` file here is imported by the main `Caddyfile`. An empty
directory contributes nothing, which is how a deployment with no extra sites
says so — there is no variable to set and no file that must exist.

This is for site blocks a particular deployment has and others do not, because a
site block cannot be made conditional: an address that expands to nothing fails
the whole configuration. Redirect-only hostnames are the usual case.

To add the redirect example, on the deployment that wants it:

```bash
cp caddy-sites/redirects.caddy.example caddy-sites/redirects.caddy
```

and edit the hostnames in it. They are written literally: a site block built
around an environment variable fails the whole configuration when the variable
is empty, and these files belong to one deployment anyway.

Only `.caddy` files are imported, so the `.example` and this README are ignored.
