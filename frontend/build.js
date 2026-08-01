const esbuild = require("esbuild");
const fs = require("fs");
const path = require("path");

const ROOT = __dirname;
const DIST = path.join(ROOT, "dist");
const isDev = process.argv.includes("--dev");
const isWatch = process.argv.includes("--watch");

const JS_LOAD_ORDER = [
  "js/state.js",
  "js/debug.js",
  "js/utils.js",
  "js/api.js",
  "js/diff.js",
  "js/preview.js",
  "js/providers.js",
  "js/messages.js",
  "js/inspector.js",
  "js/tools.js",
  "js/ws.js",
  "js/voice.js",
  "js/workspace.js",
  "js/persona.js",
  "js/settings.js",
  "js/voicemodels.js",
  "js/todos.js",
  "js/question.js",
  "js/permission.js",
  "js/skills.js",
  "js/jobs.js",
  "js/app.js",
];

const CSS_LOAD_ORDER = [
  "css/variables.css",
  "css/base.css",
  "css/sidebar.css",
  "css/header.css",
  "css/messages.css",
  "css/input.css",
  "css/modal.css",
  "css/tool.css",
  "css/diff.css",
  "css/split-pane.css",
  "css/code.css",
  "css/components.css",
  "css/todo.css",
  "css/question.css",
  "css/permission.css",
  "css/responsive.css",
  "css/skills.css",
  "css/jobs.css",
];

const COPY_FILES = [
  "js/diff-worker.js",
  "js/litegraph.min.js",
  "css/litegraph.min.css",
  "css/workflow.css",
  "css/inspector.css",
];

const COPY_DIRS = [
  "images",
  "js/workflow",
];

function clean() {
  if (fs.existsSync(DIST)) {
    fs.rmSync(DIST, { recursive: true, force: true });
  }
  fs.mkdirSync(path.join(DIST, "js"), { recursive: true });
  fs.mkdirSync(path.join(DIST, "css"), { recursive: true });
}

function copyRecursive(src, dest) {
  if (!fs.existsSync(src)) return;
  const stat = fs.statSync(src);
  if (stat.isDirectory()) {
    fs.mkdirSync(dest, { recursive: true });
    for (const entry of fs.readdirSync(src)) {
      copyRecursive(path.join(src, entry), path.join(dest, entry));
    }
  } else {
    fs.mkdirSync(path.dirname(dest), { recursive: true });
    fs.copyFileSync(src, dest);
  }
}

function copyStaticAssets() {
  for (const file of COPY_FILES) {
    const src = path.join(ROOT, file);
    if (fs.existsSync(src)) {
      const dest = path.join(DIST, file);
      fs.mkdirSync(path.dirname(dest), { recursive: true });
      fs.copyFileSync(src, dest);
    }
  }
  for (const dir of COPY_DIRS) {
    const src = path.join(ROOT, dir);
    if (fs.existsSync(src)) {
      copyRecursive(src, path.join(DIST, dir));
    }
  }
}

function generateHTML() {
  let html = fs.readFileSync(path.join(ROOT, "index.html"), "utf-8");

  html = html.replace(
    /<link rel="stylesheet" href="\/static\/css\/[^"]+">\n?/g,
    ""
  );
  html = html.replace(
    /<script defer src="\/static\/js\/[^"]+"><\/script>\n?/g,
    ""
  );

  html = html.replace(
    "</head>",
    '    <link rel="stylesheet" href="/static/css/app.css">\n</head>'
  );
  html = html.replace(
    "</body>",
    '    <script defer src="/static/js/app.js"></script>\n</body>'
  );

  fs.writeFileSync(path.join(DIST, "index.html"), html);
}

async function build() {
  console.log(
    isWatch ? "Starting watch build..." : isDev ? "Building (dev)..." : "Building (production)..."
  );

  clean();
  copyStaticAssets();

  const sharedConfig = {
    bundle: true,
    sourcemap: isDev || isWatch,
    minify: !isDev && !isWatch,
    target: ["es2020"],
    logLevel: "info",
  };

  const jsConfig = {
    ...sharedConfig,
    stdin: {
      contents: JS_LOAD_ORDER.map((f) => `import "./${f}";`).join("\n"),
      resolveDir: ROOT,
      sourcefile: "entry.js",
      loader: "js",
    },
    outfile: path.join(DIST, "js/app.js"),
    format: "iife",
  };

  const cssConfig = {
    ...sharedConfig,
    stdin: {
      contents: CSS_LOAD_ORDER.map((f) => `@import "./${f}";`).join("\n"),
      resolveDir: ROOT,
      sourcefile: "entry.css",
      loader: "css",
    },
    outfile: path.join(DIST, "css/app.css"),
  };

  if (isWatch) {
    const jsCtx = await esbuild.context(jsConfig);
    const cssCtx = await esbuild.context(cssConfig);
    await Promise.all([jsCtx.watch(), cssCtx.watch()]);

    generateHTML();
    console.log("Watching for changes... (Ctrl+C to stop)");

    process.on("SIGINT", async () => {
      await Promise.all([jsCtx.dispose(), cssCtx.dispose()]);
      process.exit(0);
    });
  } else {
    await Promise.all([esbuild.build(jsConfig), esbuild.build(cssConfig)]);
    generateHTML();

    const jsStat = fs.statSync(path.join(DIST, "js/app.js"));
    const cssStat = fs.statSync(path.join(DIST, "css/app.css"));
    console.log(
      `Done: app.js ${(jsStat.size / 1024).toFixed(1)}KB, app.css ${(cssStat.size / 1024).toFixed(1)}KB`
    );
  }
}

build().catch((err) => {
  console.error("Build failed:", err);
  process.exit(1);
});
