// Swagger UI on the device's own API document (docs/api.md). Replaces the
// swagger-ui-dist copy, which points at the petstore. "Try it out" runs on
// this origin; a claimed device wants its token under "Authorize".
window.onload = function () {
  window.ui = SwaggerUIBundle({
    url: "/api/v1/openapi.json",
    dom_id: "#swagger-ui",
    deepLinking: true,
    presets: [SwaggerUIBundle.presets.apis, SwaggerUIStandalonePreset],
    plugins: [SwaggerUIBundle.plugins.DownloadUrl],
    layout: "StandaloneLayout",
  });
};
