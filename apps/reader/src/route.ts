export type Route =
  | { readonly kind: "home" }
  | { readonly kind: "read"; readonly editionId: string };

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

export function parseRoute(pathname: string): Route {
  const match = /^\/read\/([^/]+)\/?$/.exec(pathname);
  const id = match?.[1];
  if (id !== undefined && UUID_RE.test(id)) return { kind: "read", editionId: id };
  return { kind: "home" };
}
