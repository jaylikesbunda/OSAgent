const assert = require('node:assert/strict');
const test = require('node:test');

let Window;

test.before(async () => {
    ({ Window } = await import('happy-dom'));
    const window = new Window({ url: 'http://localhost/' });
    global.window = window;
    global.document = window.document;
    global.navigator = window.navigator;
    global.Node = window.Node;
    global.HTMLElement = window.HTMLElement;
    global.SVGElement = window.SVGElement;
    global.requestAnimationFrame = (fn) => { fn(0); return 1; };
    global.cancelAnimationFrame = () => {};
    global.OSA = window.OSA = {};

    require('../js/state.js');
    require('../js/utils.js');
    require('../js/diagrams.js');
    require('../js/messages.js');
    require('../js/tools.js');
    require('../js/transcript.js');

    // Stubs the rest of the transcript needs so a tool card can be patched.
    OSA.stripSpeakBlock = (text) => text || '';
    OSA.stripToolCallMarkup = (text) => text || '';
    OSA.getShowThinkingBlocks = () => false;
    OSA.formatToolOutput = () => '';
    OSA.isContextTool = () => false;
});

test.beforeEach(() => {
    document.body.replaceChildren();
});

function spec() {
    return {
        kind: 'diagram',
        version: 1,
        title: 'Auth flow',
        source: 'structured',
        theme: 'dark',
        node_count: 3,
        edge_count: 2,
        spec: {
            direction: 'TB',
            nodes: [
                { id: 'login', label: 'Login', description: 'User submits credentials' },
                { id: 'mfa', label: 'MFA check', description: 'Second factor' },
                { id: 'session', label: 'Session issued' },
            ],
            edges: [
                { from: 'login', to: 'mfa', label: 'verified' },
                { from: 'mfa', to: 'session', style: 'dashed' },
            ],
            groups: [{ id: 'g1', label: 'Edge' }],
        },
    };
}

test('structured layout layers nodes and keeps every node inside the bounds', () => {
    const layout = OSA.Diagram.computeLayout(spec().spec);
    assert.equal(layout.horizontal, false);
    assert.equal(layout.nodes.length, 3);
    assert.ok(layout.width > 0 && layout.height > 0);
    // Each edge advances a rank, so the three nodes stack with no overlap.
    const byId = new Map(layout.nodes.map((node) => [node.id, node]));
    assert.ok(byId.get('mfa').y > byId.get('login').y);
    assert.ok(byId.get('session').y > byId.get('mfa').y);
    layout.nodes.forEach((node) => {
        assert.ok(node.x + node.width <= layout.width, `${node.id} inside width`);
        assert.ok(node.y + node.height <= layout.height, `${node.id} inside height`);
    });
    assert.equal(layout.edges.length, 2);
    assert.equal(layout.edges[1].dashed, true);
});

test('left-to-right direction lays the chain out across the x axis', () => {
    const horizontal = JSON.parse(JSON.stringify(spec().spec));
    horizontal.direction = 'LR';
    const layout = OSA.Diagram.computeLayout(horizontal);
    assert.equal(layout.horizontal, true);
    const byId = new Map(layout.nodes.map((node) => [node.id, node]));
    assert.ok(byId.get('mfa').x > byId.get('login').x);
});

test('a cyclic spec still terminates instead of looping forever', () => {
    const cyclic = {
        direction: 'TB',
        nodes: [{ id: 'a', label: 'A' }, { id: 'b', label: 'B' }, { id: 'c', label: 'C' }],
        edges: [
            { from: 'a', to: 'b' },
            { from: 'b', to: 'c' },
            { from: 'c', to: 'a' },
        ],
    };
    const layout = OSA.Diagram.computeLayout(cyclic);
    assert.equal(layout.nodes.length, 3);
    assert.ok(layout.height > 0);
});

test('scene building produces real SVG nodes without innerHTML injection', () => {
    const hostile = JSON.parse(JSON.stringify(spec()));
    hostile.spec.nodes[0].label = '<script>alert(1)</script>';
    const scene = OSA.Diagram.buildScene(hostile, {});
    assert.ok(scene);
    assert.equal(scene.svg.nodeName.toLowerCase(), 'svg');
    assert.equal(scene.nodes.length, 3);
    // The label is rendered as text (wrapped across tspans/text nodes), never
    // parsed as markup: no script element exists and the payload is inert.
    const rendered = Array.from(scene.svg.querySelectorAll('text'))
        .map((t) => t.textContent)
        .join(' ');
    assert.match(rendered, /alert\(1\)/);
    assert.equal(scene.svg.querySelectorAll('script').length, 0);
    assert.equal(scene.nodes[0].element.querySelector('script'), null);
});

test('raw SVG is allowlisted: scripts, handlers, and remote refs are stripped', () => {
    const raw = OSA.Diagram.sanitizeRawSvg(
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 50" onload="steal()">'
        + '<script>alert(1)</script>'
        + '<foreignObject><div>html</div></foreignObject>'
        + '<a href="https://evil.example/x"><rect width="10" height="10"/></a>'
        + '<g onclick="alert(2)"><circle cx="5" cy="5" r="4" fill="red"/></g>'
        + '</svg>'
    );
    assert.ok(raw);
    assert.equal(raw.nodeName.toLowerCase(), 'svg');
    assert.equal(raw.querySelectorAll('script').length, 0);
    assert.equal(raw.querySelectorAll('foreignObject').length, 0);
    assert.equal(raw.hasAttribute('onload'), false);
    // Links are not part of the allowlist at all, so a remote href can never
    // reach the document even in fragment form.
    assert.equal(raw.querySelectorAll('a').length, 0);
    const group = raw.querySelector('g');
    assert.equal(group.hasAttribute('onclick'), false);
    // The harmless shape survives.
    assert.equal(raw.querySelectorAll('circle').length, 1);
});

test('raw SVG keeps fragment references and rejects non-SVG documents', () => {
    const kept = OSA.Diagram.sanitizeRawSvg(
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10">'
        + '<defs><linearGradient id="g"><stop offset="0"/></linearGradient></defs>'
        + '<rect width="10" height="10" fill="url(#g)"/></svg>'
    );
    assert.ok(kept);
    assert.equal(kept.querySelector('rect').getAttribute('fill'), 'url(#g)');

    // A url() that points anywhere but a fragment is dropped.
    const remotePaint = OSA.Diagram.sanitizeRawSvg(
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10">'
        + '<rect width="10" height="10" fill="url(https://evil.example/x)"/></svg>'
    );
    assert.equal(remotePaint.querySelector('rect').getAttribute('fill'), null);

    assert.equal(OSA.Diagram.sanitizeRawSvg('<div>not an svg</div>'), null);
    assert.equal(OSA.Diagram.sanitizeRawSvg(''), null);
    assert.equal(OSA.Diagram.sanitizeRawSvg(null), null);
});

test('raw SVG keeps label text but drops stray text outside <text>', () => {
    // Regression guard: the sanitizer used to strip character data from every
    // element, silently blanking every label in a hand-authored diagram.
    const cleaned = OSA.Diagram.sanitizeRawSvg(
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 40">'
        + '<text x="10" y="20" font-size="12">kept label</text>'
        + '<title>accessible name</title>'
        + 'loose text<rect width="5" height="5"/></svg>'
    );
    assert.equal(cleaned.querySelector('text').textContent, 'kept label');
    assert.equal(cleaned.querySelector('title').textContent, 'accessible name');
    assert.equal(cleaned.textContent, 'kept labelaccessible name');
});

test('remote image references never survive sanitizing', () => {
    const cleaned = OSA.Diagram.sanitizeRawSvg(
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10">'
        + '<image href="https://evil.example/pixel.png" width="10" height="10"/></svg>'
    );
    assert.equal(cleaned.querySelectorAll('image').length, 0);
});

test('a raw diagram mounts its shapes exactly once', () => {
    // Regression guard: importing the sanitized tree used to re-append the
    // same first child forever, hanging the tab with unbounded allocations.
    const host = document.createElement('div');
    document.body.appendChild(host);
    const viewer = OSA.Diagram.mount(host, {
        kind: 'diagram',
        version: 1,
        title: 'Raw',
        source: 'raw',
        theme: 'dark',
        svg: '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 100">'
            + '<defs><linearGradient id="g"><stop offset="0" stop-color="#4f8ef7"/></linearGradient></defs>'
            + '<rect x="10" y="20" width="80" height="60" rx="8" fill="url(#g)"/>'
            + '<text x="50" y="55" text-anchor="middle" fill="#fff" font-size="12">a</text>'
            + '<circle cx="150" cy="50" r="25" fill="none" stroke="#35b46a" stroke-width="2"/>'
            + '</svg>',
    }, {});
    assert.ok(viewer);
    const world = host.querySelector('.diagram-world');
    assert.equal(world.querySelectorAll('rect').length, 1);
    assert.equal(world.querySelectorAll('circle').length, 1);
    assert.equal(world.querySelectorAll('text').length, 1);
    // The gradient def came along, so the fill still paints.
    assert.equal(world.querySelector('rect').getAttribute('fill'), 'url(#g)');
    assert.equal(world.querySelectorAll('defs linearGradient').length, 1);
    assert.match(host.querySelector('.diagram-meta').textContent, /raw SVG/);
});

test('mounting a diagram builds the card, toolbar, viewport, and hint', () => {
    const host = document.createElement('div');
    document.body.appendChild(host);
    const viewer = OSA.Diagram.mount(host, spec(), {});
    assert.ok(viewer);
    assert.ok(host.querySelector('.diagram-card'));
    assert.equal(host.querySelector('.diagram-title').textContent, 'Auth flow');
    assert.equal(host.querySelector('.diagram-meta').textContent, '3 shapes · 2 links');
    assert.ok(host.querySelector('.diagram-viewport svg.diagram-svg'));
    const actions = Array.from(host.querySelectorAll('.diagram-btn')).map((b) => b.dataset.action);
    assert.deepEqual(actions, ['zoom-out', 'zoom-in', 'fit', 'copy', 'download', 'expand']);
    assert.ok(host.querySelector('.diagram-hint').textContent.includes('Ctrl'));
});

test('clicking a node opens the inspector with its description', () => {
    const host = document.createElement('div');
    document.body.appendChild(host);
    OSA.Diagram.mount(host, spec(), {});
    const inspector = host.querySelector('.diagram-inspector');
    assert.equal(inspector.hidden, true);

    const node = host.querySelector('[data-diagram-node-id="login"]');
    assert.ok(node);
    assert.equal(node.getAttribute('tabindex'), '0');
    assert.equal(node.getAttribute('role'), 'button');
    // A tap that never moved is a selection, not a pan.
    node.dispatchEvent(new window.Event('pointerup', { bubbles: true }));

    assert.equal(inspector.hidden, false);
    assert.equal(inspector.querySelector('.diagram-inspector-title').textContent, 'Login');
    assert.equal(inspector.querySelector('.diagram-inspector-body').textContent, 'User submits credentials');
    assert.ok(node.classList.contains('is-selected'));

    inspector.querySelector('.diagram-inspector-close').click();
    assert.equal(inspector.hidden, true);
    assert.equal(node.classList.contains('is-selected'), false);
});

test('a node without a description still gets an inspector', () => {
    const host = document.createElement('div');
    document.body.appendChild(host);
    OSA.Diagram.mount(host, spec(), {});
    const node = host.querySelector('[data-diagram-node-id="session"]');
    node.dispatchEvent(new window.Event('pointerup', { bubbles: true }));
    const inspector = host.querySelector('.diagram-inspector');
    assert.equal(inspector.hidden, false);
    assert.match(inspector.querySelector('.diagram-inspector-body').textContent, /No further detail/);
});

test('download falls back to an anchor when the save picker is unavailable', async () => {
    const created = [];
    const originalCreate = window.URL.createObjectURL;
    window.URL.createObjectURL = (blob) => { created.push(blob); return 'blob:diagram'; };
    window.URL.revokeObjectURL = () => {};
    try {
        const host = document.createElement('div');
        document.body.appendChild(host);
        OSA.Diagram.mount(host, spec(), {});
        const download = host.querySelector('.diagram-btn[data-action="download"]');
        download.click();
        // The picker path is async; let the promise chain settle.
        await new Promise((resolve) => setImmediate(resolve));
        assert.equal(created.length, 1);
        assert.equal(created[0].type, 'image/svg+xml;charset=utf-8');
    } finally {
        window.URL.createObjectURL = originalCreate;
    }
});

test('save uses the file picker so the user chooses the location', async () => {
    const written = [];
    let offeredName = '';
    window.showSaveFilePicker = async (options) => {
        offeredName = options.suggestedName;
        return {
            createWritable: async () => ({
                write: async (text) => { written.push(text); },
                close: async () => {},
            }),
        };
    };
    try {
        const host = document.createElement('div');
        document.body.appendChild(host);
        OSA.Diagram.mount(host, spec(), {});
        host.querySelector('.diagram-btn[data-action="download"]').click();
        await new Promise((resolve) => setImmediate(resolve));
        assert.equal(offeredName, 'auth-flow.svg');
        assert.equal(written.length, 1);
        assert.match(written[0], /^<\?xml version="1\.0" encoding="UTF-8"\?>/);
    } finally {
        delete window.showSaveFilePicker;
    }
});

test('cancelling the save picker is reported, not treated as a failure', async () => {
    let flashes = [];
    window.showSaveFilePicker = async () => {
        const err = new Error('aborted');
        err.name = 'AbortError';
        throw err;
    };
    try {
        const host = document.createElement('div');
        document.body.appendChild(host);
        OSA.Diagram.mount(host, spec(), {});
        const save = host.querySelector('.diagram-btn[data-action="download"]');
        save.click();
        await new Promise((resolve) => setImmediate(resolve));
        flashes.push(save.textContent);
        assert.equal(flashes[0], 'Cancelled');
    } finally {
        delete window.showSaveFilePicker;
    }
});

test('accented nodes are tinted, never striped with a solid edge bar', () => {
    const accented = JSON.parse(JSON.stringify(spec()));
    accented.spec.nodes[0].kind = 'primary';
    accented.spec.nodes[0].group = '';
    accented.spec.groups = [];
    // Mounted rather than built standalone: happy-dom's selector engine is
    // unreliable when the query root is an <svg> element.
    const accentedHost = document.createElement('div');
    document.body.appendChild(accentedHost);
    OSA.Diagram.mount(accentedHost, accented, {});
    const node = accentedHost.querySelector('.diagram-node');
    const rects = Array.from(node.querySelectorAll('rect'));
    // The body plus the top-edge sheen, and nothing narrow: a stripe down the
    // left edge would show up as a rect only a few units wide.
    rects.forEach((rect) => {
        assert.ok(Number(rect.getAttribute('width')) > 40, 'no narrow accent bar');
    });
    // The second rect is the top-edge sheen, inset by one pixel on each side.
    assert.equal(
        Number(rects[0].getAttribute('width')) - Number(rects[1].getAttribute('width')),
        2
    );
    // The tint is a solid blended fill (never #RRGGBBAA, which not every SVG
    // consumer understands), so the accent reads as a wash rather than a line.
    assert.match(rects[0].getAttribute('fill'), /^#[0-9a-f]{6}$/i);
    assert.match(rects[0].getAttribute('stroke'), /^#[0-9a-f]{6}$/i);

    const grouped = JSON.parse(JSON.stringify(spec()));
    grouped.spec.groups = [{ id: 'g1', label: 'Edge', color: 'blue' }];
    grouped.spec.nodes[0].group = 'g1';
    grouped.spec.nodes[1].group = 'g1';
    const groupedHost = document.createElement('div');
    document.body.appendChild(groupedHost);
    OSA.Diagram.mount(groupedHost, grouped, {});
    const group = groupedHost.querySelector('.diagram-group');
    assert.ok(group, 'group backdrop present');
    assert.equal(group.querySelectorAll('rect').length, 1, 'no left colour bar on group backdrops');});

test('an unrenderable diagram degrades to a visible message instead of throwing', () => {
    const host = document.createElement('div');
    document.body.appendChild(host);
    const viewer = OSA.Diagram.mount(host, { kind: 'diagram', title: 'Broken' }, {});
    assert.equal(viewer, null);
    assert.match(host.querySelector('.diagram-error').textContent, /could not be rendered/);
});

test('diagram tool cards render as a standalone card with no tool chrome', () => {
    OSA.transcriptView = {
        toolNodesByCallId: new Map(),
        ctxNodesByCallId: new Map(),
        wrapperNodesByKey: new Map(),
    };
    OSA.getTranscriptView = () => OSA.transcriptView;

    const running = {
        kind: 'tool',
        key: 'tool:call-d',
        callId: 'call-d',
        toolName: 'draw_diagram',
        args: { title: 'Auth flow', spec: spec().spec },
        output: '',
        title: '',
        prelude: '',
        status: 'running',
        success: false,
        completed: false,
        metadata: null,
        context: false,
    };
    const container = OSA.buildToolCardElement(running);
    document.body.appendChild(container);
    OSA.patchToolCardElement(container, running);

    // No tool row to open: the diagram sits in the message column directly.
    assert.equal(container.querySelector('.tool-trigger-inline'), null);
    assert.equal(container.querySelector('.tool-chevron'), null);
    assert.equal(container.querySelector('.tool-args'), null);
    assert.equal(container.querySelector('.tool-body'), null);
    assert.ok(container.classList.contains('diagram-container'));
    assert.match(container.querySelector('.diagram-pending-label').textContent, /Drawing diagram/);
    assert.ok(container.querySelector('.diagram-pending-spinner'), 'pending shows a spinner, not a giant empty box');

    running.completed = true;
    running.success = true;
    running.status = 'done';
    running.title = 'Auth flow';
    running.output = 'Diagram drawn.';
    running.metadata = spec();
    OSA.patchToolCardElement(container, running);

    assert.ok(container.querySelector('.diagram-card .diagram-svg'));
    assert.equal(container.querySelector('.diagram-title').textContent, 'Auth flow');
    // Completion replaces the pending state entirely: no placeholder can
    // survive next to the mounted diagram.
    assert.equal(container.querySelector('.diagram-pending'), null);
    assert.equal(container.querySelectorAll('.diagram-card').length, 1);
    // Still no collapsible chrome after completion.
    assert.equal(container.querySelector('.tool-trigger-inline'), null);
});

test('a stale running event never re-adds the placeholder over a mounted diagram', () => {
    OSA.transcriptView = {
        toolNodesByCallId: new Map(),
        ctxNodesByCallId: new Map(),
        wrapperNodesByKey: new Map(),
    };
    OSA.getTranscriptView = () => OSA.transcriptView;

    const item = {
        kind: 'tool',
        key: 'tool:call-stale',
        callId: 'call-stale',
        toolName: 'draw_diagram',
        args: { title: 'Auth flow' },
        output: '',
        title: '',
        prelude: '',
        status: 'done',
        success: true,
        completed: true,
        metadata: spec(),
        context: false,
    };
    const container = OSA.buildToolCardElement(item);
    document.body.appendChild(container);
    OSA.patchToolCardElement(container, item);
    assert.ok(container.querySelector('.diagram-card'));

    // A replayed out-of-order start event must not push a placeholder into the
    // host above or below the mounted viewer.
    item.completed = false;
    item.status = 'running';
    item.metadata = null;
    OSA.patchToolCardElement(container, item);
    assert.equal(container.querySelector('.diagram-pending'), null);
    assert.equal(container.querySelectorAll('.diagram-card').length, 1);
    assert.ok(container.querySelector('.diagram-card .diagram-svg'));
});

test('a tool unit signature changes when completion metadata arrives', () => {
    const before = {
        type: 'tool',
        key: 'tool:call-s',
        items: [{
            kind: 'tool',
            toolName: 'draw_diagram',
            status: 'running',
            completed: false,
            output: '',
            title: '',
            prelude: '',
            metadata: null,
        }],
    };
    const after = {
        type: 'tool',
        key: 'tool:call-s',
        items: [{
            kind: 'tool',
            toolName: 'draw_diagram',
            status: 'done',
            completed: true,
            output: 'ok',
            title: 'Auth flow',
            prelude: '',
            metadata: spec(),
        }],
    };
    assert.notEqual(OSA.unitSignature(before), OSA.unitSignature(after));
});

test('background tool events keep diagram metadata on completion', () => {
    const entry = { tools: [{ tool_call_id: 'call-bg', tool_name: 'draw_diagram', metadata: null, completed: false }] };
    OSA.upsertEntryToolEvent(entry, { tool_call_id: 'call-bg', tool_name: 'draw_diagram' }, false);
    OSA.upsertEntryToolEvent(entry, {
        tool_call_id: 'call-bg',
        tool_name: 'draw_diagram',
        success: true,
        output: 'drawn',
        title: 'Auth flow',
        metadata: spec(),
    }, true);
    assert.equal(entry.tools[0].completed, true);
    assert.equal(entry.tools[0].metadata.title, 'Auth flow');
});

test('a raw diagram keeps a non-zero viewBox origin instead of cropping it', () => {
    const scene = OSA.Diagram.buildScene({
        kind: 'diagram',
        title: 'Offset',
        source: 'raw',
        svg: '<svg xmlns="http://www.w3.org/2000/svg" viewBox="20 30 200 100">'
            + '<rect x="20" y="30" width="200" height="100" fill="#123456"/></svg>',
    }, {});
    assert.equal(scene.width, 200);
    assert.equal(scene.height, 100);
    assert.equal(scene.svg.getAttribute('viewBox'), '20 30 200 100');
    const bg = scene.svg.querySelector('rect');
    assert.equal(bg.getAttribute('x'), '20');
    assert.equal(bg.getAttribute('y'), '30');
});

test('adjacent group backdrops keep a consistent breathing gap', () => {
    // Two groups sharing nodes across lanes: their frames must not kiss.
    // Regression guard for backdrops that used to abut with no clear space.
    const grouped = {
        title: 'Groups',
        spec: {
            direction: 'LR',
            nodes: [
                { id: 'a', label: 'A', group: 'g1' },
                { id: 'b', label: 'B', group: 'g1' },
                { id: 'c', label: 'C', group: 'g2' },
                { id: 'd', label: 'D', group: 'g2' },
                { id: 'e', label: 'E', group: 'g2' },
            ],
            edges: [
                { from: 'a', to: 'c' },
                { from: 'c', to: 'e' },
            ],
            groups: [
                { id: 'g1', label: 'One', color: 'blue' },
                { id: 'g2', label: 'Two', color: 'green' },
            ],
        },
    };
    const layout = OSA.Diagram.computeLayout(grouped.spec);
    const byId = new Map(layout.nodes.map((node) => [node.id, node]));
    layout.groups.forEach((frame) => {
        // Every declared member sits inside its own backdrop.
        const members = grouped.spec.nodes.filter((n) => n.group === frame.id);
        members.forEach((member) => {
            const box = byId.get(member.id);
            assert.ok(
                box.x >= frame.x
                && box.x + box.width <= frame.x + frame.width
                && box.y >= frame.y
                && box.y + box.height <= frame.y + frame.height,
                member.id + ' stays inside its backdrop',
            );
        });
    });
    // The backdrops on the cross axis keep a clear gap (no shared edge, no kiss).
    const sorted = layout.groups.slice().sort((a, b) => a.x - b.x || a.y - b.y);
    for (let i = 1; i < sorted.length; i += 1) {
        const prev = sorted[i - 1];
        const frame = sorted[i];
        const xOverlap = Math.min(prev.x + prev.width, frame.x + frame.width) - Math.max(prev.x, frame.x);
        if (xOverlap > 0) {
            const clearance = frame.y - (prev.y + prev.height);
            assert.ok(clearance >= 18, 'vertical clearance between backdrops >= 18, got ' + clearance);
        }
    }
});

test('remounting a diagram releases the previous view observers', () => {
    const host = document.createElement('div');
    document.body.appendChild(host);
    const first = OSA.Diagram.mount(host, spec(), {});
    assert.ok(first);
    const firstCleanup = host._diagramCleanup;
    assert.equal(typeof firstCleanup, 'function');
    OSA.Diagram.mount(host, spec(), {});
    // The old cleanup ran on re-mount and the card is rebuilt exactly once.
    assert.notEqual(host.querySelectorAll('.diagram-card').length, 2);
    assert.equal(host.querySelectorAll('.diagram-card').length, 1);
});

test('slugify produces a safe download filename', () => {
    assert.equal(OSA.Diagram.slugify('Auth Flow / v2!'), 'auth-flow-v2');
    assert.equal(OSA.Diagram.slugify(''), 'diagram');
});
