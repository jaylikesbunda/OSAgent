window.OSA = window.OSA || {};

// Inline SVG diagram viewer for the `draw_diagram` tool.
//
// Two shapes of input are supported, both produced by the backend tool:
//   * `metadata.spec` — a validated node/edge spec that this file lays out.
//     Explicit attributes (not CSS classes) are used for every fill/stroke so
//     the exported .svg is byte-for-byte the picture on screen.
//   * `metadata.svg`  — hand-authored raw SVG, re-sanitized here even though
//     the backend already rejected the dangerous parts. The browser is the
//     authoritative parse, so this allowlist pass is the one that matters.
//
// Nothing in this file injects model output through innerHTML. Structured
// content is built with createElement/textContent; raw SVG is rebuilt from a
// DOMParser tree restricted to a small allowlist of elements and attributes.
OSA.Diagram = (function() {
    'use strict';

    const TOOL_NAME = 'draw_diagram';
    const SVG_NS = 'http://www.w3.org/2000/svg';
    const MIN_SCALE = 0.15;
    const MAX_SCALE = 6;

    let sceneSeq = 0;

    // ---- palettes -------------------------------------------------------
    // Self-contained (no CSS variables) so an exported diagram renders the
    // same in a browser, an image viewer, or an editor.
    const PALETTES = {
        dark: {
            canvas: '#171a1f',
            groupFill: 'rgba(255,255,255,0.035)',
            groupStroke: 'rgba(255,255,255,0.13)',
            groupText: 'rgba(255,255,255,0.5)',
            nodeFill: '#272c34',
            nodeStroke: 'rgba(255,255,255,0.14)',
            nodeSheen: 'rgba(255,255,255,0.07)',
            nodeText: '#eef1f5',
            edge: 'rgba(238,241,245,0.45)',
            edgeStrong: 'rgba(238,241,245,0.62)',
            labelFill: '#22272e',
            labelStroke: 'rgba(255,255,255,0.1)',
            labelText: 'rgba(238,241,245,0.82)',
            accent: '#e0575f',
        },
        light: {
            canvas: '#eef1f6',
            groupFill: 'rgba(15,23,42,0.035)',
            groupStroke: 'rgba(15,23,42,0.12)',
            groupText: 'rgba(15,23,42,0.5)',
            nodeFill: '#ffffff',
            nodeStroke: 'rgba(15,23,42,0.16)',
            nodeSheen: 'rgba(15,23,42,0.04)',
            nodeText: '#10151d',
            edge: 'rgba(16,21,29,0.38)',
            edgeStrong: 'rgba(16,21,29,0.55)',
            labelFill: '#ffffff',
            labelStroke: 'rgba(15,23,42,0.1)',
            labelText: 'rgba(16,21,29,0.75)',
            accent: '#c2333c',
        },
    };

    const KIND_COLORS = {
        primary: '#4f8ef7',
        success: '#35b46a',
        warning: '#d99a1c',
        danger: '#d94f4f',
        info: '#4aa5c4',
        muted: '#7c8794',
    };

    const GROUP_COLORS = {
        blue: '#4f8ef7',
        violet: '#8a6ce0',
        green: '#35b46a',
        amber: '#d99a1c',
        red: '#d94f4f',
        teal: '#33a99b',
        pink: '#d062a4',
    };

    // ---- raw SVG allowlist ----------------------------------------------
    // Element names are matched case-insensitively: XML parsing preserves the
    // camelCase of `linearGradient`, but some DOM implementations fold it.
    const ALLOWED_ELEMENTS = new Set([
        'svg', 'g', 'defs', 'title', 'desc', 'style',
        'path', 'rect', 'circle', 'ellipse', 'line', 'polyline', 'polygon',
        'text', 'tspan', 'marker', 'lineargradient', 'radialgradient', 'stop',
        'clippath',
    ]);

    const ALLOWED_ATTRIBUTES = new Set([
        'id', 'class', 'viewbox', 'width', 'height', 'x', 'y', 'x1', 'y1', 'x2', 'y2',
        'cx', 'cy', 'r', 'rx', 'ry', 'dx', 'dy',
        'd', 'points', 'transform', 'preserveaspectratio', 'viewbox',
        'fill', 'fill-opacity', 'stroke', 'stroke-width', 'stroke-opacity',
        'stroke-dasharray', 'stroke-dashoffset', 'stroke-linecap', 'stroke-linejoin',
        'opacity', 'font-family', 'font-size', 'font-weight', 'font-style',
        'text-anchor', 'dominant-baseline', 'letter-spacing', 'textlength',
        'markerwidth', 'markerheight', 'refx', 'refy', 'orient', 'markerunits',
        'gradientunits', 'spreadmethod', 'offset', 'stop-color', 'stop-opacity',
        'shape-rendering', 'vector-effect', 'paint-order', 'color',
    ]);

    // ---- helpers ---------------------------------------------------------
    function isDarkDocument() {
        try {
            const styles = window.getComputedStyle(document.documentElement);
            const raw = (styles.getPropertyValue('--p-bg') || '').trim();
            const hex = raw.replace('#', '');
            if (/^[0-9a-f]{6}$/i.test(hex)) {
                const r = parseInt(hex.slice(0, 2), 16) / 255;
                const g = parseInt(hex.slice(2, 4), 16) / 255;
                const b = parseInt(hex.slice(4, 6), 16) / 255;
                const lin = (c) => (c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4));
                const lum = 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
                return lum < 0.4;
            }
        } catch (err) { /* fall through */ }
        try {
            if (window.matchMedia && window.matchMedia('(prefers-color-scheme: light)').matches) {
                return false;
            }
        } catch (err) { /* fall through */ }
        return true;
    }

    function resolveTheme(theme) {
        if (theme === 'dark' || theme === 'light') return theme;
        return isDarkDocument() ? 'dark' : 'light';
    }

    function svgEl(name, attrs) {
        const el = document.createElementNS(SVG_NS, name);
        if (attrs) {
            Object.keys(attrs).forEach(function(key) {
                const value = attrs[key];
                if (value === null || value === undefined) return;
                el.setAttribute(key, String(value));
            });
        }
        return el;
    }

    // Blend two hex colours into a solid one. Tints are computed here rather
    // than emitted as #RRGGBBAA so an exported file renders identically in
    // every SVG consumer, not just browsers.
    function mix(base, over, ratio) {
        const parse = function(value) {
            const hex = String(value || '').replace('#', '');
            if (!/^[0-9a-f]{6}$/i.test(hex)) return null;
            return [
                parseInt(hex.slice(0, 2), 16),
                parseInt(hex.slice(2, 4), 16),
                parseInt(hex.slice(4, 6), 16),
            ];
        };
        const from = parse(base);
        const to = parse(over);
        if (!from || !to) return over;
        const amount = Math.max(0, Math.min(1, ratio));
        const channel = function(i) {
            const value = Math.round(from[i] + (to[i] - from[i]) * amount);
            return Math.max(0, Math.min(255, value)).toString(16).padStart(2, '0');
        };
        return '#' + channel(0) + channel(1) + channel(2);
    }

    function slugify(text) {
        const slug = String(text || 'diagram')
            .toLowerCase()
            .replace(/[^a-z0-9]+/g, '-')
            .replace(/^-+|-+$/g, '');
        return slug || 'diagram';
    }

    function wrapLabel(text, maxChars) {
        const words = String(text || '').split(/\s+/).filter(Boolean);
        if (!words.length) return [''];
        const lines = [];
        let current = '';
        words.forEach(function(word) {
            if (!current) {
                current = word;
            } else if ((current + ' ' + word).length <= maxChars) {
                current += ' ' + word;
            } else {
                lines.push(current);
                current = word;
            }
            // A single word longer than the budget: hard-split it.
            while (current.length > maxChars) {
                lines.push(current.slice(0, maxChars));
                current = current.slice(maxChars);
            }
        });
        if (current) lines.push(current);
        return lines.slice(0, 4);
    }

    // ---- structured layout ----------------------------------------------
    function assignRanks(nodes, edges) {
        const index = new Map();
        nodes.forEach(function(node, i) { index.set(node.id, i); });
        const rank = nodes.map(function() { return 0; });
        // Relaxation pass: settles longest-path layering and terminates even
        // on cyclic input because the pass count is bounded by node count.
        const passes = Math.max(1, nodes.length);
        for (let pass = 0; pass < passes; pass += 1) {
            let changed = false;
            for (let e = 0; e < edges.length; e += 1) {
                const from = index.get(edges[e].from);
                const to = index.get(edges[e].to);
                if (from === undefined || to === undefined || from === to) continue;
                if (rank[to] < rank[from] + 1) {
                    rank[to] = rank[from] + 1;
                    changed = true;
                }
            }
            if (!changed) break;
        }
        return rank;
    }

    function nodeKindColor(node) {
        const key = String(node.kind || '').toLowerCase();
        if (KIND_COLORS[key]) return KIND_COLORS[key];
        if (key === 'accent' || key === 'start' || key === 'end') return null; // palette accent
        return null;
    }

    // ---- spacing scale ----------------------------------------------------
    // One scale for everything: node body padding, gaps between nodes, the
    // group backdrop's breathing room, and the outer canvas margin all derive
    // from it (in some cases as multiples), so no two diagrams invent their
    // own rhythm.
    const SPACE = {
        // Inside a node: label text sits this far from the shape's edge.
        nodePadX: 24,
        nodePadY: 17,
        // Per-line pitch of the node label.
        lineHeight: 20,
        // Space between two nodes in the same lane, and between lanes.
        gap: 56,
        // Space the group backdrop leaves around its members (the label lives
        // in the top band, so more headroom above than beside).
        groupPadX: 16,
        groupPadTop: 40,
        groupPadBottom: 18,
        // Minimum clearance between two group backdrops so adjacent groups
        // never kiss. Also the clearance behind any group at the canvas edge.
        groupGap: 18,
        // Canvas margin around every shape and backdrop.
        framePad: 32,
    };

    function computeLayout(spec) {
        const nodes = (spec && Array.isArray(spec.nodes)) ? spec.nodes : [];
        const edges = (spec && Array.isArray(spec.edges)) ? spec.edges : [];
        const groups = (spec && Array.isArray(spec.groups)) ? spec.groups : [];
        const horizontal = String((spec && spec.direction) || 'TB').toUpperCase() === 'LR';

        const NODE_MIN_W = 184;
        const NODE_MAX_W = 268;
        const NODE_H = 60;

        const boxes = nodes.map(function(node, i) {
            const lines = wrapLabel(node.label, 22);
            const widest = lines.reduce(function(max, line) { return Math.max(max, line.length); }, 0);
            const width = Math.min(
                NODE_MAX_W,
                Math.max(
                    NODE_MIN_W,
                    Number(node.width) || 0,
                    widest * 8.6 + 2 * SPACE.nodePadX
                )
            );
            // Balanced body: equal padding above the first line and below the
            // last, regardless of how many lines wrap.
            const textBlock = lines.length * SPACE.lineHeight;
            const height = Math.max(NODE_H, textBlock + 2 * SPACE.nodePadY);
            return {
                id: String(node.id),
                label: String(node.label || ''),
                description: String(node.description || ''),
                kind: String(node.kind || ''),
                group: node.group ? String(node.group) : '',
                icon: node.icon ? String(node.icon) : '',
                index: i,
                lines: lines,
                width: width,
                height: height,
                fixedX: Number.isFinite(Number(node.x)) ? Number(node.x) : null,
                fixedY: Number.isFinite(Number(node.y)) ? Number(node.y) : null,
            };
        });

        const byId = new Map(boxes.map(function(box) { return [box.id, box]; }));
        const validEdges = edges.filter(function(edge) {
            return edge && byId.has(String(edge.from)) && byId.has(String(edge.to));
        });

        const rank = assignRanks(boxes, validEdges);
        const lanes = new Map();
        boxes.forEach(function(box, i) {
            if (box.fixedX !== null && box.fixedY !== null) return;
            const r = rank[i];
            if (!lanes.has(r)) lanes.set(r, []);
            lanes.get(r).push(box);
        });

        // When a frame boundary sits between two ranks, the backdrop pads on
        // either side of the boundary must fit inside the gap plus a clear
        // stop, so stacked groups never touch.
        const frameBoundaryGap = SPACE.groupPadTop + SPACE.groupPadBottom + SPACE.groupGap;
        const laneKeys = Array.from(lanes.keys()).sort(function(a, b) { return a - b; });
        let cross = 0;
        let lastPairGap = SPACE.gap;
        laneKeys.forEach(function(key, laneIndex) {
            const lane = lanes.get(key);
            let main = 0;
            lane.forEach(function(box) {
                main = Math.max(main, horizontal ? box.width : box.height);
            });
            const nextLane = laneIndex + 1 < laneKeys.length ? lanes.get(laneKeys[laneIndex + 1]) : null;
            const boundaryBetweenFrames = lane.some(function(box) { return box.group; })
                || (nextLane ? nextLane.some(function(box) { return box.group; }) : false);
            const pairGap = boundaryBetweenFrames
                ? Math.max(SPACE.gap, frameBoundaryGap)
                : SPACE.gap;
            cross = Math.max(cross, main);
            if (!horizontal) {
                let running = 0;
                lane.forEach(function(box) {
                    box.x = running;
                    box.y = cross;
                    running += box.width + SPACE.gap;
                });
            } else {
                let running = 0;
                lane.forEach(function(box) {
                    box.x = cross;
                    box.y = running;
                    running += box.height + SPACE.gap;
                });
            }
            cross += main + pairGap;
            lastPairGap = pairGap;
        });

        // Center each lane on the cross axis, then apply the fixed overrides.
        const span = Math.max(0, cross - lastPairGap);
        laneKeys.forEach(function(key) {
            lanes.get(key).forEach(function(box) {
                if (horizontal) {
                    const laneHeight = lanes.get(key).reduce(function(sum, b) { return sum + b.height; }, 0)
                        + SPACE.gap * (lanes.get(key).length - 1);
                    box.y = box.y + (span - laneHeight) / 2;
                } else {
                    const laneWidth = lanes.get(key).reduce(function(sum, b) { return sum + b.width; }, 0)
                        + SPACE.gap * (lanes.get(key).length - 1);
                    box.x = box.x + (span - laneWidth) / 2;
                }
            });
        });
        boxes.forEach(function(box) {
            if (box.fixedX !== null) box.x = box.fixedX;
            if (box.fixedY !== null) box.y = box.fixedY;
            if (box.x === undefined) box.x = 0;
            if (box.y === undefined) box.y = 0;
        });

        // Normalize so every group frame has the same clear margin to the
        // canvas edge as the shapes do. Without the group pads here, a frame
        // wrapped around the first or last rank would run past the canvas.
        const anyGrouped = boxes.some(function(box) { return !!box.group; });
        const padTop = SPACE.framePad + (anyGrouped ? SPACE.groupPadTop : 0);
        const padLeft = SPACE.framePad + (anyGrouped ? SPACE.groupPadX : 0);
        const originX = Math.min.apply(null, boxes.map(function(b) { return b.x; }));
        const originY = Math.min.apply(null, boxes.map(function(b) { return b.y; }));
        boxes.forEach(function(box) {
            box.x += padLeft - originX;
            box.y += padTop - originY;
        });

        // Group backdrops behind their members.
        const groupBoxes = groups.map(function(group) {
            const members = boxes.filter(function(box) { return box.group === String(group.id); });
            if (!members.length) return null;
            const minX = Math.min.apply(null, members.map(function(b) { return b.x; }));
            const minY = Math.min.apply(null, members.map(function(b) { return b.y; }));
            const maxX = Math.max.apply(null, members.map(function(b) { return b.x + b.width; }));
            const maxY = Math.max.apply(null, members.map(function(b) { return b.y + b.height; }));
            return {
                id: String(group.id),
                label: String(group.label || ''),
                color: GROUP_COLORS[String(group.color || '').toLowerCase()] || null,
                x: minX - SPACE.groupPadX,
                y: minY - SPACE.groupPadTop,
                width: (maxX - minX) + 2 * SPACE.groupPadX,
                height: (maxY - minY) + SPACE.groupPadTop + SPACE.groupPadBottom,
            };
        }).filter(Boolean);

        // Safety net: a group can wrap more than one rank, which makes its
        // backdrop reach toward its neighbour's. Sweep the backdrops in main
        // axis order and push the later one (with its members, so the shapes
        // stay nested and edges keep meeting the boxes) until every pair of
        // stacked backdrops keeps one clear gap stop.
        const placedFrames = [];
        const mainOf = function(frame) { return horizontal ? frame.x : frame.y; };
        const sizeOf = function(frame) { return horizontal ? frame.width : frame.height; };
        const shiftGroupMembers = function(groupId, amount) {
            if (!amount || !Number.isFinite(amount)) return;
            boxes.forEach(function(box) {
                if (box.group !== groupId) return;
                if (horizontal) {
                    box.x += amount;
                } else {
                    box.y += amount;
                }
            });
        };
        [...groupBoxes]
            .sort(function(a, b) { return mainOf(a) - mainOf(b); })
            .forEach(function(frame) {
                let start = mainOf(frame);
                placedFrames.forEach(function(prev) {
                    // Only pairs stacked along the main axis constrain each
                    // other; side-by-side frames already get their clearance
                    // from the lane gap.
                    const crossOverlap = horizontal
                        ? Math.min(prev.y + prev.height, frame.y + frame.height) - Math.max(prev.y, frame.y)
                        : Math.min(prev.x + prev.width, frame.x + frame.width) - Math.max(prev.x, frame.x);
                    if (crossOverlap > 0) {
                        start = Math.max(start, mainOf(prev) + sizeOf(prev) + SPACE.groupGap);
                    }
                });
                const deficit = start - mainOf(frame);
                if (deficit > 0) {
                    if (horizontal) {
                        frame.x += deficit;
                    } else {
                        frame.y += deficit;
                    }
                    shiftGroupMembers(frame.id, deficit);
                }
                placedFrames.push(frame);
            });

        const all = boxes.concat(groupBoxes);
        const width = Math.max(
            120,
            Math.max.apply(null, all.map(function(b) { return b.x + b.width; })) + SPACE.framePad
        );
        const height = Math.max(
            120,
            Math.max.apply(null, all.map(function(b) { return b.y + b.height; })) + SPACE.framePad
        );

        const geometry = validEdges.map(function(edge) {
            const a = byId.get(String(edge.from));
            const b = byId.get(String(edge.to));
            // Rounded-elbow routing reads as a diagram; a single cubic spline
            // between two off-centre boxes bulges and crosses other content.
            const stub = 18;
            const corner = 9;
            let d;
            let lx;
            let ly;
            if (horizontal) {
                const x1 = a.x + a.width;
                const y1 = a.y + a.height / 2;
                const x2 = b.x;
                const y2 = b.y + b.height / 2;
                const run = x2 - x1;
                if (Math.abs(y2 - y1) < 1.5 || run < stub * 2) {
                    d = 'M ' + x1 + ' ' + y1 + ' L ' + x2 + ' ' + y2;
                    lx = (x1 + x2) / 2;
                    ly = (y1 + y2) / 2;
                } else {
                    const midX = x1 + run / 2;
                    const dir = y2 > y1 ? 1 : -1;
                    d = 'M ' + x1 + ' ' + y1
                        + ' L ' + (midX - corner) + ' ' + y1
                        + ' Q ' + midX + ' ' + y1 + ' ' + midX + ' ' + (y1 + corner * dir)
                        + ' L ' + midX + ' ' + (y2 - corner * dir)
                        + ' Q ' + midX + ' ' + y2 + ' ' + (midX + corner) + ' ' + y2
                        + ' L ' + x2 + ' ' + y2;
                    lx = midX;
                    ly = (y1 + y2) / 2;
                }
            } else {
                const x1 = a.x + a.width / 2;
                const y1 = a.y + a.height;
                const x2 = b.x + b.width / 2;
                const y2 = b.y;
                const run = y2 - y1;
                if (Math.abs(x2 - x1) < 1.5 || run < stub * 2) {
                    d = 'M ' + x1 + ' ' + y1 + ' L ' + x2 + ' ' + y2;
                    lx = (x1 + x2) / 2;
                    ly = (y1 + y2) / 2;
                } else {
                    const midY = y1 + run / 2;
                    const dir = x2 > x1 ? 1 : -1;
                    d = 'M ' + x1 + ' ' + y1
                        + ' L ' + x1 + ' ' + (midY - corner)
                        + ' Q ' + x1 + ' ' + midY + ' ' + (x1 + corner * dir) + ' ' + midY
                        + ' L ' + (x2 - corner * dir) + ' ' + midY
                        + ' Q ' + x2 + ' ' + midY + ' ' + x2 + ' ' + (midY + corner)
                        + ' L ' + x2 + ' ' + y2;
                    lx = (x1 + x2) / 2;
                    ly = midY;
                }
            }
            return {
                from: String(edge.from),
                to: String(edge.to),
                label: edge.label ? String(edge.label) : '',
                dashed: String(edge.style || '').toLowerCase() === 'dashed',
                d: d,
                lx: lx,
                ly: ly,
            };
        });

        return {
            horizontal: horizontal,
            width: width,
            height: height,
            nodes: boxes,
            edges: geometry,
            groups: groupBoxes,
        };
    }

    // ---- structured scene -------------------------------------------------
    function buildStructuredScene(spec, palette, theme, interactive) {
        const layout = computeLayout(spec);
        const idPrefix = 'dg' + (sceneSeq += 1);
        const svg = svgEl('svg', {
            xmlns: SVG_NS,
            'xmlns:xlink': 'http://www.w3.org/1999/xlink',
            viewBox: '0 0 ' + layout.width + ' ' + layout.height,
            role: 'img',
            'aria-label': (spec && spec.title) || 'Diagram',
        });
        svg.classList.add('diagram-svg');

        const defs = svgEl('defs');
        const marker = svgEl('marker', {
            id: idPrefix + '-arrow',
            viewBox: '0 0 10 10',
            refX: '8.5',
            refY: '5',
            markerWidth: '6',
            markerHeight: '6',
            orient: 'auto-start-reverse',
        });
        marker.appendChild(svgEl('path', {
            d: 'M 0.5 1.4 L 9 5 L 0.5 8.6 z',
            fill: palette.edgeStrong,
        }));
        defs.appendChild(marker);
        svg.appendChild(defs);

        const bg = svgEl('rect', {
            x: 0, y: 0, width: layout.width, height: layout.height, fill: palette.canvas,
        });
        svg.appendChild(bg);

        const world = svgEl('g', { class: 'diagram-world' });
        svg.appendChild(world);

        layout.groups.forEach(function(group) {
            const g = svgEl('g', { class: 'diagram-group' });
            g.appendChild(svgEl('rect', {
                x: group.x, y: group.y, width: group.width, height: group.height,
                rx: 18, ry: 18,
                fill: group.color ? mix(palette.canvas, group.color, 0.07) : palette.groupFill,
                stroke: group.color ? mix(palette.canvas, group.color, 0.4) : palette.groupStroke,
                'stroke-width': 1.1,
            }));
            const label = svgEl('text', {
                // The label sits in the backdrop's own top band, midway down
                // it — the same rhythm as the rest of the scale.
                x: group.x + SPACE.groupPadX + 2,
                y: group.y + SPACE.groupPadTop / 2 + 2,
                fill: group.color || palette.groupText,
                'font-family': 'Inter, system-ui, sans-serif',
                'font-size': 11,
                'font-weight': 600,
                'letter-spacing': '0.1em',
            });
            // Uppercase here rather than with CSS: `text-transform` is not a
            // presentation attribute SVG renderers honor, and the export has
            // to match what the screen shows.
            label.textContent = group.label.toUpperCase();
            g.appendChild(label);
            world.appendChild(g);
        });

        const edgeLayer = svgEl('g', { class: 'diagram-edges' });
        layout.edges.forEach(function(edge) {
            const g = svgEl('g', { class: 'diagram-edge' });
            g.appendChild(svgEl('path', {
                d: edge.d,
                fill: 'none',
                stroke: palette.edge,
                'stroke-width': 1.7,
                'stroke-linecap': 'round',
                'stroke-linejoin': 'round',
                'stroke-dasharray': edge.dashed ? '5 6' : null,
                'marker-end': 'url(#' + idPrefix + '-arrow)',
            }));
            if (edge.label) {
                const width = edge.label.length * 6.6 + 16;
                g.appendChild(svgEl('rect', {
                    x: edge.lx - width / 2, y: edge.ly - 10, width: width, height: 20,
                    rx: 6, ry: 6,
                    fill: palette.labelFill,
                    stroke: palette.labelStroke,
                    'stroke-width': 1,
                }));
                const text = svgEl('text', {
                    x: edge.lx, y: edge.ly + 4,
                    'text-anchor': 'middle',
                    fill: palette.labelText,
                    'font-family': 'Inter, system-ui, sans-serif',
                    'font-size': 11.5,
                    'font-weight': 500,
                });
                text.textContent = edge.label;
                g.appendChild(text);
            }
            edgeLayer.appendChild(g);
        });
        world.appendChild(edgeLayer);

        const nodeLayer = svgEl('g', { class: 'diagram-nodes' });
        const interactiveNodes = [];
        layout.nodes.forEach(function(node) {
            const accent = nodeKindColor(node);
            const g = svgEl('g', {
                class: 'diagram-node',
                'data-diagram-node-id': node.id,
            });
            if (interactive) {
                g.setAttribute('tabindex', '0');
                g.setAttribute('role', 'button');
                g.setAttribute('aria-label', node.label);
            }
            // A role-tinted shape, not a solid stripe down the left edge: the
            // earlier accent bar read as a rendering artifact rather than
            // decoration.
            g.appendChild(svgEl('rect', {
                x: node.x, y: node.y, width: node.width, height: node.height,
                rx: 14, ry: 14,
                fill: accent ? mix(palette.canvas, accent, 0.15) : palette.nodeFill,
                stroke: accent ? mix(palette.canvas, accent, 0.9) : palette.nodeStroke,
                'stroke-width': accent ? 1.4 : 1.1,
            }));
            // A one-pixel sheen along the top edge gives the card a little
            // depth without the cost (or editor support) of a filter.
            g.appendChild(svgEl('rect', {
                x: node.x + 1, y: node.y + 1, width: node.width - 2, height: node.height - 2,
                rx: 13, ry: 13,
                fill: 'none',
                stroke: palette.nodeSheen,
                'stroke-width': 1,
            }));
            const startY = node.y + node.height / 2 - (node.lines.length - 1) * (SPACE.lineHeight / 2) + 5.5;
            node.lines.forEach(function(line, lineIndex) {
                const text = svgEl('text', {
                    x: node.x + node.width / 2,
                    y: startY + lineIndex * SPACE.lineHeight,
                    'text-anchor': 'middle',
                    fill: palette.nodeText,
                    'font-family': 'Inter, system-ui, sans-serif',
                    'font-size': 14.5,
                    'font-weight': 600,
                });
                text.textContent = line;
                g.appendChild(text);
            });
            if (node.icon) {
                g.appendChild(svgEl('rect', {
                    x: node.x + node.width - 26, y: node.y + 9, width: 17, height: 17,
                    rx: 5, ry: 5, fill: accent || palette.nodeStroke, opacity: '0.9',
                }));
                const icon = svgEl('text', {
                    x: node.x + node.width - 17.5, y: node.y + 21.5,
                    'text-anchor': 'middle',
                    fill: palette.nodeText,
                    'font-family': 'Inter, system-ui, sans-serif',
                    'font-size': 10,
                    'font-weight': 700,
                });
                icon.textContent = node.icon.slice(0, 2);
                g.appendChild(icon);
            }
            nodeLayer.appendChild(g);
            interactiveNodes.push({ id: node.id, element: g, node: node });
        });
        world.appendChild(nodeLayer);

        return {
            svg: svg,
            width: layout.width,
            height: layout.height,
            nodes: interactiveNodes,
            interactive: !!interactive,
        };
    }

    // ---- raw SVG sanitizing ---------------------------------------------
    // Elements whose character data is content, not layout noise.
    const TEXT_ELEMENTS = new Set(['text', 'tspan', 'title', 'desc', 'style']);

    function sanitizeRawSvg(source) {
        if (!source || typeof source !== 'string') return null;
        let doc = null;
        try {
            doc = new window.DOMParser().parseFromString(source, 'image/svg+xml');
        } catch (err) {
            return null;
        }
        if (!doc || !doc.documentElement) return null;
        if (doc.documentElement.nodeName.toLowerCase() === 'parsererror') return null;
        const root = doc.documentElement;
        if (String(root.localName || root.nodeName || '').toLowerCase() !== 'svg') return null;

        const walk = function(el) {
            const parentTag = String(el.localName || el.nodeName || '').toLowerCase();
            const keepsText = TEXT_ELEMENTS.has(parentTag);
            const children = Array.prototype.slice.call(el.childNodes);
            children.forEach(function(child) {
                if (child.nodeType !== 1) {
                    // Character data is content inside <text>/<title>/<desc>
                    // and <style>; anywhere else it is stray markup text.
                    if (child.nodeType === 3) {
                        if (!keepsText && String(child.nodeValue || '').trim()) {
                            el.removeChild(child);
                        }
                    } else {
                        el.removeChild(child);
                    }
                    return;
                }
                const name = String(child.localName || child.nodeName || '').toLowerCase();
                if (!ALLOWED_ELEMENTS.has(name)) {
                    el.removeChild(child);
                    return;
                }
                if (name === 'style') {
                    const css = String(child.textContent || '');
                    const lowered = css.toLowerCase();
                    if (lowered.indexOf('javascript:') !== -1
                        || lowered.indexOf('expression(') !== -1
                        || lowered.indexOf('@import') !== -1
                        || /url\(\s*(?!['"]?#)/.test(lowered)) {
                        el.removeChild(child);
                        return;
                    }
                }
                Array.prototype.slice.call(child.attributes).forEach(function(attr) {
                    const attrName = attr.name.toLowerCase();
                    const attrValue = attr.value || '';
                    if (attrName.indexOf('on') === 0) { child.removeAttribute(attr.name); return; }
                    if (attrName === 'href' || attrName === 'xlink:href') {
                        if (attrValue.trim().charAt(0) !== '#') child.removeAttribute(attr.name);
                        return;
                    }
                    // A paint/fill/stroke may only reference a local fragment.
                    if (/url\(/i.test(attrValue) && !/url\(\s*['"]?#/i.test(attrValue)) {
                        child.removeAttribute(attr.name);
                        return;
                    }
                    if (!ALLOWED_ATTRIBUTES.has(attrName)) child.removeAttribute(attr.name);
                });
                walk(child);
            });
        };
        walk(root);

        root.removeAttribute('onload');
        return root;
    }

    // Attribute lookup that tolerates a parser which folded the camelCase of
    // `viewBox` to `viewbox`.
    function attr(el, name) {
        const direct = el.getAttribute(name);
        if (direct !== null && direct !== undefined) return direct;
        const lower = el.getAttribute(name.toLowerCase());
        return lower === null || lower === undefined ? null : lower;
    }

    function buildRawScene(svgSource, palette, title, interactive) {
        const root = sanitizeRawSvg(svgSource);
        if (!root) return null;
        // Keep the source viewBox verbatim when it is usable: a hand-authored
        // diagram with a non-zero origin would otherwise be cropped.
        const viewBox = String(attr(root, 'viewBox') || '')
            .split(/[\s,]+/)
            .filter(Boolean)
            .map(Number);
        const usableViewBox = viewBox.length === 4 && viewBox.every(function(n) { return Number.isFinite(n); })
            && viewBox[2] > 0 && viewBox[3] > 0;
        let width = parseFloat(attr(root, 'width'));
        let height = parseFloat(attr(root, 'height'));
        if (usableViewBox) {
            width = viewBox[2];
            height = viewBox[3];
        }
        if (!Number.isFinite(width) || !Number.isFinite(height) || width <= 0 || height <= 0) {
            width = 800;
            height = 600;
        }
        const svg = svgEl('svg', {
            xmlns: SVG_NS,
            'xmlns:xlink': 'http://www.w3.org/1999/xlink',
            viewBox: usableViewBox
                ? viewBox.join(' ')
                : '0 0 ' + width + ' ' + height,
            role: 'img',
            'aria-label': title || 'Diagram',
        });
        svg.classList.add('diagram-svg', 'diagram-svg-raw');
        const bg = svgEl('rect', {
            x: usableViewBox ? viewBox[0] : 0,
            y: usableViewBox ? viewBox[1] : 0,
            width: width,
            height: height,
            fill: palette.canvas,
        });
        svg.appendChild(bg);
        const world = svgEl('g', { class: 'diagram-world' });
        // The allowlist walk already removed every active construct; re-import
        // through DOM so the mounted tree belongs to this document. The source
        // children are snapshotted first: importNode copies, so advancing
        // `root.firstChild` while appending would never terminate.
        Array.prototype.slice.call(root.childNodes).forEach(function(child) {
            world.appendChild(document.importNode(child, true));
        });
        svg.appendChild(world);

        const interactiveNodes = [];
        if (interactive) {
            // Only leaf shapes become click targets: a bare <g> would swallow
            // its children's clicks, and a `fill="none"` backdrop is not a
            // shape a reader can see.
            Array.prototype.slice.call(world.querySelectorAll('rect,circle,ellipse,path,polygon,polyline,line,text')).forEach(function(el, i) {
                const tag = String(el.localName || el.nodeName || '').toLowerCase();
                const fill = String(attr(el, 'fill') || '').trim().toLowerCase();
                const stroke = String(attr(el, 'stroke') || '').trim();
                if (tag !== 'text' && tag !== 'line' && tag !== 'polyline' && (fill === 'none' || fill === 'transparent') && !stroke) {
                    return;
                }
                el.classList.add('diagram-node');
                el.setAttribute('tabindex', '0');
                el.setAttribute('role', 'button');
                el.setAttribute('data-diagram-node-id', 'shape-' + i);
                interactiveNodes.push({
                    id: 'shape-' + i,
                    element: el,
                    node: { label: 'Shape ' + (i + 1), description: '' },
                });
            });
        }
        return { svg: svg, width: width, height: height, nodes: interactiveNodes, interactive: !!interactive };
    }

    function buildScene(metadata, options) {
        const opts = options || {};
        const theme = resolveTheme(metadata && metadata.theme);
        const palette = PALETTES[theme] || PALETTES.dark;
        const title = (metadata && metadata.title) || 'Diagram';
        const interactive = opts.interactive !== false;

        if (metadata && typeof metadata.svg === 'string' && metadata.svg.trim()) {
            const raw = buildRawScene(metadata.svg, palette, title, interactive);
            if (raw) {
                raw.theme = theme;
                raw.title = title;
                raw.palette = palette;
                return raw;
            }
        }
        const spec = (metadata && metadata.spec) || { nodes: [] };
        if (!Array.isArray(spec.nodes) || !spec.nodes.length) return null;
        const scene = buildStructuredScene(spec, palette, theme, interactive);
        scene.theme = theme;
        scene.title = title;
        scene.palette = palette;
        return scene;
    }

    // ---- viewer ------------------------------------------------------------
    function iconButton(label, title, glyph) {
        const button = document.createElement('button');
        button.type = 'button';
        button.className = 'diagram-btn';
        button.title = title;
        button.setAttribute('aria-label', title);
        button.dataset.action = label;
        button.textContent = glyph;
        return button;
    }

    function mount(host, metadata, options) {
        const opts = options || {};
        if (!host) return null;
        // Re-mounting happens whenever the tool's metadata changes. Tear the
        // previous viewer's observers and window listeners down first so a
        // long chat cannot accumulate one per re-render.
        if (typeof host._diagramCleanup === 'function') {
            try { host._diagramCleanup(); } catch (err) { /* ignore */ }
            host._diagramCleanup = null;
        }
        const scene = buildScene(metadata, opts);
        host.replaceChildren();
        if (!scene) {
            const empty = document.createElement('div');
            empty.className = 'diagram-error';
            empty.textContent = 'This diagram could not be rendered.';
            host.appendChild(empty);
            return null;
        }

        const card = document.createElement('div');
        card.className = 'diagram-card' + (opts.expanded ? ' is-expanded' : '');

        const header = document.createElement('div');
        header.className = 'diagram-header';
        const titleEl = document.createElement('div');
        titleEl.className = 'diagram-title';
        titleEl.textContent = scene.title;
        header.appendChild(titleEl);

        const meta = document.createElement('div');
        meta.className = 'diagram-meta';
        const nodeCount = (metadata && metadata.node_count) || (scene.nodes && scene.nodes.length) || 0;
        const edgeCount = (metadata && metadata.edge_count)
            || (metadata && metadata.spec && Array.isArray(metadata.spec.edges) ? metadata.spec.edges.length : 0);
        const parts = [];
        if (nodeCount) parts.push(nodeCount + (nodeCount === 1 ? ' shape' : ' shapes'));
        if (edgeCount) parts.push(edgeCount + (edgeCount === 1 ? ' link' : ' links'));
        if (metadata && metadata.source === 'raw') parts.push('raw SVG');
        if (parts.length) {
            meta.textContent = parts.join(' · ');
            header.appendChild(meta);
        }

        const toolbar = document.createElement('div');
        toolbar.className = 'diagram-toolbar';
        const zoomOut = iconButton('zoom-out', 'Zoom out', '−');
        const zoomLabel = document.createElement('span');
        zoomLabel.className = 'diagram-zoom-label';
        zoomLabel.textContent = '100%';
        const zoomIn = iconButton('zoom-in', 'Zoom in', '+');
        const fit = iconButton('fit', 'Fit to view', 'Fit');
        const copy = iconButton('copy', 'Copy SVG source', 'Copy');
        const download = iconButton('download', 'Download SVG', 'Save');
        const expand = iconButton('expand', 'Expand diagram', 'Expand');
        [zoomOut, zoomLabel, zoomIn, fit, copy, download, expand].forEach(function(el) {
            toolbar.appendChild(el);
        });
        header.appendChild(toolbar);
        card.appendChild(header);

        const viewport = document.createElement('div');
        viewport.className = 'diagram-viewport';
        viewport.setAttribute('role', 'application');
        viewport.setAttribute('aria-label', 'Diagram canvas. Drag to pan, ctrl+wheel to zoom.');
        // The SVG carries the scene viewBox, so the browser already fits the
        // diagram to the viewport. Zoom/pan therefore rides on a CSS transform
        // of the element rather than an inner transform, which would otherwise
        // scale the same content twice.
        viewport.style.background = scene.palette.canvas;
        viewport.appendChild(scene.svg);
        card.appendChild(viewport);

        const hint = document.createElement('div');
        hint.className = 'diagram-hint';
        hint.textContent = 'Drag to pan \u00b7 Ctrl or Cmd + wheel, or pinch, to zoom \u00b7 click a shape for detail';
        card.appendChild(hint);

        const inspector = document.createElement('div');
        inspector.className = 'diagram-inspector';
        inspector.hidden = true;
        card.appendChild(inspector);

        host.appendChild(card);

        const state = { scale: 1, x: 0, y: 0, node: null, viewportHeight: 0 };
        const pointers = new Map();

        function applyTransform() {
            scene.svg.style.transformOrigin = '0 0';
            scene.svg.style.transform = 'translate(' + state.x + 'px,' + state.y + 'px) scale(' + state.scale + ')';
            zoomLabel.textContent = Math.round(state.scale * 100) + '%';
        }

        function clampScale(scale) {
            return Math.max(MIN_SCALE, Math.min(MAX_SCALE, scale));
        }

        function fitToView() {
            // The viewBox already fits the scene, so "fit" is the identity
            // transform. It is still worth re-running after a resize because
            // the caller may have zoomed or panned in the meantime.
            state.scale = 1;
            state.x = 0;
            state.y = 0;
            applyTransform();
        }

        // A tall, narrow diagram fitted into a fixed box shrinks its own type to
        // nothing, and a short, wide one leaves dead space. Give the viewport
        // the shape it needs instead, within bounds, so both stay legible.
        function autoSizeViewport() {
            if (card.classList.contains('is-expanded')) return;
            const rect = viewport.getBoundingClientRect();
            if (!rect.width) return;
            const aspect = scene.width / Math.max(1, scene.height);
            const next = Math.round(Math.max(150, Math.min(520, rect.width / aspect)));
            if (Math.abs(next - state.viewportHeight) < 6) return;
            state.viewportHeight = next;
            viewport.style.height = next + 'px';
        }

        function zoomAt(factor, clientX, clientY) {
            const rect = viewport.getBoundingClientRect();
            const px = Number.isFinite(clientX) ? clientX - rect.left : rect.width / 2;
            const py = Number.isFinite(clientY) ? clientY - rect.top : rect.height / 2;
            const next = clampScale(state.scale * factor);
            const ratio = next / state.scale;
            // Keep the point under the cursor fixed in element coordinates.
            state.x = px - (px - state.x) * ratio;
            state.y = py - (py - state.y) * ratio;
            state.scale = next;
            applyTransform();
        }

        function resetToHundred() {
            fitToView();
        }

        function selectNode(entry) {
            if (state.node && state.node.element) state.node.element.classList.remove('is-selected');
            state.node = entry;
            if (!entry) {
                inspector.hidden = true;
                return;
            }
            entry.element.classList.add('is-selected');
            inspector.replaceChildren();
            const title = document.createElement('div');
            title.className = 'diagram-inspector-title';
            title.textContent = entry.node.label || entry.id;
            inspector.appendChild(title);
            if (entry.node.description) {
                const body = document.createElement('div');
                body.className = 'diagram-inspector-body';
                body.textContent = entry.node.description;
                inspector.appendChild(body);
            } else {
                const body = document.createElement('div');
                body.className = 'diagram-inspector-body';
                body.textContent = 'No further detail was provided for this shape.';
                inspector.appendChild(body);
            }
            const close = document.createElement('button');
            close.type = 'button';
            close.className = 'diagram-inspector-close';
            close.textContent = '×';
            close.title = 'Close detail';
            close.setAttribute('aria-label', 'Close detail');
            close.addEventListener('click', function() { selectNode(null); });
            inspector.appendChild(close);
            inspector.hidden = false;
        }

        function flash(button, message) {
            const original = button.textContent;
            button.textContent = message;
            button.classList.add('is-flashing');
            setTimeout(function() {
                button.textContent = original;
                button.classList.remove('is-flashing');
            }, 1400);
        }

        function standaloneSvgString() {
            const clone = buildScene(metadata, { interactive: false });
            if (!clone) return null;
            clone.svg.setAttribute('xmlns', SVG_NS);
            clone.svg.setAttribute('xmlns:xlink', 'http://www.w3.org/1999/xlink');
            clone.svg.setAttribute('width', String(Math.round(clone.width)));
            clone.svg.setAttribute('height', String(Math.round(clone.height)));
            clone.svg.removeAttribute('class');
            Array.prototype.slice.call(clone.svg.querySelectorAll('[tabindex]')).forEach(function(el) {
                el.removeAttribute('tabindex');
                el.removeAttribute('role');
            });
            const body = new window.XMLSerializer().serializeToString(clone.svg);
            return '<?xml version="1.0" encoding="UTF-8"?>\n' + body;
        }

        // Prefer the File System Access API so the user actually gets a save
        // dialog and picks the location, instead of a blob silently landing in
        // the downloads folder. Falls back to an anchor download where the API
        // is missing or the pick was refused for another reason.
        async function saveSvgFile(text, filename) {
            if (typeof window.showSaveFilePicker === 'function') {
                try {
                    const handle = await window.showSaveFilePicker({
                        suggestedName: filename,
                        types: [{ description: 'SVG image', accept: { 'image/svg+xml': ['.svg'] } }],
                    });
                    const writable = await handle.createWritable();
                    await writable.write(text);
                    await writable.close();
                    return 'saved';
                } catch (err) {
                    const name = err && err.name;
                    if (name === 'AbortError') return 'cancelled';
                    if (name === 'SecurityError' || name === 'NotAllowedError') return 'unavailable';
                    // Any other failure falls through to the anchor path.
                }
            }
            const blob = new window.Blob([text], { type: 'image/svg+xml;charset=utf-8' });
            const url = window.URL.createObjectURL(blob);
            const link = document.createElement('a');
            link.href = url;
            link.download = filename;
            document.body.appendChild(link);
            link.click();
            link.remove();
            setTimeout(function() { window.URL.revokeObjectURL(url); }, 2000);
            return 'saved';
        }

        toolbar.addEventListener('click', function(event) {
            const button = event.target.closest ? event.target.closest('.diagram-btn') : null;
            if (!button) return;
            event.preventDefault();
            const action = button.dataset.action;
            if (action === 'zoom-in') zoomAt(1.25);
            else if (action === 'zoom-out') zoomAt(0.8);
            else if (action === 'fit') {
                autoSizeViewport();
                fitToView();
            } else if (action === 'copy') {
                const text = standaloneSvgString();
                if (text && navigator.clipboard && navigator.clipboard.writeText) {
                    navigator.clipboard.writeText(text)
                        .then(function() { flash(button, 'Copied'); })
                        .catch(function() { flash(button, 'Failed'); });
                } else {
                    flash(button, 'Unavailable');
                }
            } else if (action === 'download') {
                const text = standaloneSvgString();
                if (!text) { flash(button, 'Failed'); return; }
                saveSvgFile(text, slugify(scene.title) + '.svg').then(function(result) {
                    if (result === 'cancelled') flash(button, 'Cancelled');
                    else if (result === 'unavailable') flash(button, 'Unavailable');
                    else flash(button, 'Saved');
                }).catch(function() { flash(button, 'Failed'); });
            } else if (action === 'expand') {
                const expanded = card.classList.toggle('is-expanded');
                button.textContent = expanded ? 'Collapse' : 'Expand';
                // Leaving expanded mode has to hand the inline height back.
                if (!expanded) autoSizeViewport();
                requestAnimationFrame(fitToView);
            }
        });

        // Pointer pan (single) and pinch zoom (two fingers).
        viewport.addEventListener('pointerdown', function(event) {
            pointers.set(event.pointerId, { x: event.clientX, y: event.clientY });
            if (pointers.size === 1) {
                viewport.dataset.dragX = String(event.clientX);
                viewport.dataset.dragY = String(event.clientY);
                viewport.dataset.moved = '0';
                viewport.classList.add('is-panning');
            }
            if (viewport.setPointerCapture) {
                try { viewport.setPointerCapture(event.pointerId); } catch (err) { /* ignore */ }
            }
        });

        viewport.addEventListener('pointermove', function(event) {
            if (!pointers.has(event.pointerId)) return;
            pointers.set(event.pointerId, { x: event.clientX, y: event.clientY });
            if (pointers.size >= 2) {
                const pts = Array.from(pointers.values());
                const dist = Math.hypot(pts[0].x - pts[1].x, pts[0].y - pts[1].y);
                const last = Number(viewport.dataset.pinchDist || 0);
                if (last > 0 && dist > 0) {
                    const cx = (pts[0].x + pts[1].x) / 2;
                    const cy = (pts[0].y + pts[1].y) / 2;
                    zoomAt(dist / last, cx, cy);
                }
                viewport.dataset.pinchDist = String(dist);
                return;
            }
            const dx = event.clientX - Number(viewport.dataset.dragX || 0);
            const dy = event.clientY - Number(viewport.dataset.dragY || 0);
            if (Math.abs(dx) > 2 || Math.abs(dy) > 2) viewport.dataset.moved = '1';
            viewport.dataset.dragX = String(event.clientX);
            viewport.dataset.dragY = String(event.clientY);
            state.x += dx;
            state.y += dy;
            applyTransform();
        });

        const endPointer = function(event) {
            pointers.delete(event.pointerId);
            if (pointers.size < 2) delete viewport.dataset.pinchDist;
            if (pointers.size === 0) {
                viewport.classList.remove('is-panning');
                const target = event.target && event.target.closest
                    ? event.target.closest('.diagram-node')
                    : null;
                if (target && viewport.dataset.moved !== '1') {
                    const entry = scene.nodes.find(function(candidate) {
                        return candidate.element === target;
                    });
                    if (entry) selectNode(entry);
                } else if (viewport.dataset.moved !== '1') {
                    selectNode(null);
                }
            }
        };
        viewport.addEventListener('pointerup', endPointer);
        viewport.addEventListener('pointercancel', endPointer);

        viewport.addEventListener('wheel', function(event) {
            // Plain wheel keeps scrolling the transcript; ctrl/⌘ + wheel (and
            // trackpad pinch, which browsers report as ctrl+wheel) zooms.
            if (!event.ctrlKey && !event.metaKey) return;
            event.preventDefault();
            zoomAt(event.deltaY < 0 ? 1.12 : 0.89, event.clientX, event.clientY);
        }, { passive: false });

        viewport.addEventListener('dblclick', function(event) {
            event.preventDefault();
            zoomAt(1.4, event.clientX, event.clientY);
        });

        viewport.addEventListener('keydown', function(event) {
            if (event.key === 'Escape') { selectNode(null); return; }
            if (event.key !== 'Enter' && event.key !== ' ') return;
            const target = event.target.closest ? event.target.closest('.diagram-node') : null;
            if (!target) return;
            event.preventDefault();
            const entry = scene.nodes.find(function(candidate) { return candidate.element === target; });
            if (entry) selectNode(entry);
        });

        // Size and fit once the element has a real box, and again on resize.
        requestAnimationFrame(function() {
            autoSizeViewport();
            fitToView();
        });
        const cleanup = [];
        if (typeof window.ResizeObserver === 'function' && window.ResizeObserver) {
            // The card, not the viewport: the viewport's height is our own
            // output, and observing it would feed back on itself.
            const observer = new window.ResizeObserver(function() {
                autoSizeViewport();
                fitToView();
            });
            observer.observe(card);
            cleanup.push(function() { observer.disconnect(); });
        } else {
            const onResize = function() {
                autoSizeViewport();
                fitToView();
            };
            window.addEventListener('resize', onResize);
            cleanup.push(function() { window.removeEventListener('resize', onResize); });
        }
        host._diagramCleanup = function() {
            cleanup.forEach(function(fn) { fn(); });
        };

        applyTransform();
        return {
            card: card,
            scene: scene,
            fit: fitToView,
            reset: resetToHundred,
            destroy: function() {
                if (typeof host._diagramCleanup === 'function') host._diagramCleanup();
                host._diagramCleanup = null;
                card.remove();
            },
        };
    }

    return {
        TOOL_NAME: TOOL_NAME,
        isTool: function(name) { return name === TOOL_NAME; },
        isMetadata: function(metadata) {
            return !!(metadata && metadata.kind === 'diagram');
        },
        buildScene: buildScene,
        computeLayout: computeLayout,
        sanitizeRawSvg: sanitizeRawSvg,
        mount: mount,
        slugify: slugify,
        paletteFor: function(theme) { return PALETTES[resolveTheme(theme)]; },
    };
})();
