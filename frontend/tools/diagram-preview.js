// Dev-only: renders a draw_diagram spec to a standalone SVG + a card preview
// page, so the viewer output can be eyeballed outside the app.
const fs = require('fs');
const path = require('path');

const { Window } = require('happy-dom');

const MARK = path.join(__dirname, 'preview.log');
try { fs.unlinkSync(MARK); } catch (err) { /* ignore */ }
const mark = (msg) => { fs.appendFileSync(MARK, msg + '\n'); };

const JS_DIR = path.join(__dirname, '..', 'js');
const window = new Window({ url: 'http://localhost/' });
global.window = window;
global.document = window.document;
global.navigator = window.navigator;
global.requestAnimationFrame = (fn) => { fn(0); return 1; };
global.OSA = window.OSA = {};

require(path.join(JS_DIR, 'state.js'));
require(path.join(JS_DIR, 'utils.js'));
require(path.join(JS_DIR, 'diagrams.js'));

const tbSpec = {
    title: 'OSAgent architecture',
    source: 'structured',
    theme: 'dark',
    node_count: 6,
    edge_count: 6,
    spec: {
        direction: 'TB',
        nodes: [
            { id: 'ui', label: 'Web UI + Discord', group: 'front' },
            { id: 'ws', label: 'WebSocket events', group: 'front' },
            { id: 'runtime', label: 'Agent runtime', description: 'Owns the tool loop, streaming, and retries.', group: 'core' },
            { id: 'registry', label: 'Tool registry', description: 'Core tools, deferred catalog, and MCP servers.', group: 'core' },
            { id: 'sqlite', label: 'SQLite', group: 'data' },
            { id: 'files', label: 'Workspace files', group: 'data' },
        ],
        edges: [
            { from: 'ui', to: 'ws', label: 'events' },
            { from: 'ws', to: 'runtime' },
            { from: 'runtime', to: 'registry', label: 'tool call' },
            { from: 'runtime', to: 'sqlite', label: 'transcript' },
            { from: 'registry', to: 'files', label: 'read/write' },
            { from: 'registry', to: 'sqlite', style: 'dashed' },
        ],
        groups: [
            { id: 'front', label: 'Interface', color: 'blue' },
            { id: 'core', label: 'Core', color: 'violet' },
            { id: 'data', label: 'Storage', color: 'teal' },
        ],
    },
};

const lrSpec = {
    title: 'Publish decision',
    source: 'structured',
    theme: 'light',
    node_count: 3,
    edge_count: 2,
    spec: {
        direction: 'LR',
        nodes: [
            { id: 'review', label: 'Review', description: 'Two approvals needed', kind: 'primary' },
            { id: 'staging', label: 'Staging', description: 'Automated smoke tests', kind: 'success' },
            { id: 'ship', label: 'Ship', description: 'Signed release', kind: 'warning' },
        ],
        edges: [
            { from: 'review', to: 'staging', label: 'approved' },
            { from: 'staging', to: 'ship', label: 'green' },
        ],
    },
};

const rawSpec = {
    title: 'Raw SVG escape hatch',
    source: 'raw',
    theme: 'dark',
    node_count: 2,
    edge_count: 1,
    svg: '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 420 160">'
        + '<defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1">'
        + '<stop offset="0" stop-color="#4f8ef7"/><stop offset="1" stop-color="#8a6ce0"/></linearGradient></defs>'
        + '<rect x="0" y="0" width="420" height="160" fill="none"/>'
        + '<rect x="20" y="45" width="150" height="70" rx="12" fill="url(#g)"/>'
        + '<text x="95" y="86" text-anchor="middle" fill="#fff" font-size="15" font-family="Inter">hand written</text>'
        + '<path d="M 175 80 L 245 80" stroke="#8b95a3" stroke-width="2" fill="none"/>'
        + '<circle cx="265" cy="80" r="30" fill="none" stroke="#35b46a" stroke-width="3"/>'
        + '<text x="265" y="86" text-anchor="middle" fill="#e9ecf1" font-size="14" font-family="Inter">kept</text>'
        + '</svg>',
};

function standalone(metadata) {
    mark('standalone ' + metadata.title);
    const scene = OSA.Diagram.buildScene(metadata, { interactive: false });
    scene.svg.setAttribute('xmlns', 'http://www.w3.org/2000/svg');
    scene.svg.setAttribute('width', String(Math.round(scene.width)));
    scene.svg.setAttribute('height', String(Math.round(scene.height)));
    scene.svg.removeAttribute('class');
    mark('serialize ' + metadata.title);
    const out = '<?xml version="1.0" encoding="UTF-8"?>\n' + new window.XMLSerializer().serializeToString(scene.svg);
    mark('standalone done ' + metadata.title + ' ' + out.length);
    return out;
}

const outDir = path.join(__dirname, '..', 'workflow_artifacts', 'diagram-preview');
fs.mkdirSync(outDir, { recursive: true });
fs.writeFileSync(path.join(outDir, 'architecture.svg'), standalone(tbSpec));
fs.writeFileSync(path.join(outDir, 'publish-decision.svg'), standalone(lrSpec));
fs.writeFileSync(path.join(outDir, 'raw-escape-hatch.svg'), standalone(rawSpec));

const sections = [
    ['OSAgent architecture (TB, grouped, dark)', tbSpec],
    ['Publish decision (LR, kinds, light)', lrSpec],
    ['Raw SVG escape hatch (sanitized)', rawSpec],
].map(([label, metadata]) => {
    mark('mounting ' + label);
    const host = window.document.createElement('div');
    window.document.body.appendChild(host);
    OSA.Diagram.mount(host, metadata, {});
    host.querySelector('.diagram-card').setAttribute('data-preview', '1');
    mark('serializing ' + label);
    const card = host.innerHTML;
    mark('done ' + label + ' ' + card.length);
    return '<h2>' + label + '</h2>' + card;
}).join('\n');

// Static preview only: happy-dom reports zero-sized boxes, so the viewer's own
// fit pass cannot run at build time. This reproduces it in the browser so the
// page shows what the app will actually render.
const FIT_SCRIPT = [
    '<script>',
    'document.querySelectorAll(".diagram-card").forEach(function(card){',
    '  var svg = card.querySelector("svg");',
    '  var vp = card.querySelector(".diagram-viewport");',
    '  var vb = svg.getAttribute("viewBox").split(/[ ,]+/).map(Number);',
    '  function fit(){',
    '    var aspect = vb[2] / vb[3];',
    '    var next = Math.round(Math.max(150, Math.min(520, vp.clientWidth / aspect)));',
    '    if (Math.abs(next - (parseInt(vp.style.height, 10) || 0)) > 6) vp.style.height = next + "px";',
    '    svg.style.transformOrigin = "0 0";',
    '    svg.style.transform = "translate(0px,0px) scale(1)";',
    '    card.querySelector(".diagram-zoom-label").textContent = "100%";',
    '  }',
    '  fit();',
    '  window.addEventListener("resize", fit);',
    '});',
    '<\/script>',
].join('\n');

const diagramCss = fs.readFileSync(path.join(__dirname, '..', 'css', 'diagram.css'), 'utf-8');

const page = '<!DOCTYPE html><html><head><meta charset="utf-8"><title>diagram preview</title>'
    + '<style>' + diagramCss + '</style>'
    + '<style>:root{--p-bg:#313131;--p-surface:#414141;--p-muted:#525252;--p-accent:#ca3e47;'
    + '--bg-primary:var(--p-bg);--bg-secondary:var(--p-surface);--bg-tertiary:var(--p-muted);'
    + '--bg-hover:#666;--border:rgba(255,255,255,.12);--border-hover:rgba(255,255,255,.2);'
    + '--text-primary:rgba(255,255,255,.93);--text-secondary:rgba(255,255,255,.68);'
    + '--text-muted:rgba(255,255,255,.4);--accent:var(--p-accent);--accent-glow:rgba(202,62,71,.34);'
    + '--radius-md:10px;--radius-sm:6px;--shadow-sm:0 1px 2px rgba(0,0,0,.3);'
    + '--font-sans:Inter,system-ui,sans-serif;}'
    + 'body{font-family:Inter,system-ui,sans-serif;background:#313131;color:#fff;padding:20px;}'
    + 'h2{font-size:14px;font-weight:600;margin:18px 0 8px;opacity:.8}'
    + '.diagram-card{max-width:760px}'
    + '</style></head><body>' + sections + FIT_SCRIPT + '</body></html>';
fs.writeFileSync(path.join(outDir, 'preview.html'), page, 'utf-8');

console.log('wrote', outDir);
console.log('architecture.svg', fs.statSync(path.join(outDir, 'architecture.svg')).size, 'bytes');
console.log('publish-decision.svg', fs.statSync(path.join(outDir, 'publish-decision.svg')).size, 'bytes');
console.log('raw-escape-hatch.svg', fs.statSync(path.join(outDir, 'raw-escape-hatch.svg')).size, 'bytes');
