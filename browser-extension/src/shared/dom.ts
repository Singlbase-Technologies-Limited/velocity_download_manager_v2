/** The page's element with this id, checked to be of the kind the page expects. */
export function $<T extends HTMLElement>(id: string, kind: new () => T): T {
  const element = document.getElementById(id);
  if (!(element instanceof kind)) throw new Error(`The page has no #${id} of the expected kind.`);
  return element;
}
