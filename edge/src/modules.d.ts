// Wrangler `rules` (type "Text") imports the installers as strings, served at
// /install.sh and /install.ps1.
declare module "*.sh" {
  const text: string;
  export default text;
}
declare module "*.ps1" {
  const text: string;
  export default text;
}
