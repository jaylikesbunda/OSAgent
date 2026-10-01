window.OSA = window.OSA || {};

OSA._diffWorker = null;

OSA._ensureDiffWorker = function() {
    if (OSA._diffWorker) return OSA._diffWorker;
    OSA._diffWorker = new Worker('/static/js/diff-worker.js');
    return OSA._diffWorker;
};

OSA.computeLineDiff = function(oldText, newText) {
    return window.OSADiff.compute(oldText, newText);
};

OSA._diffJobs = new Map();
OSA._diffJobSequence = 0;
OSA.computeLineDiffAsync = function(oldText, newText) {
    const left = typeof oldText === 'string' ? oldText : '';
    const right = typeof newText === 'string' ? newText : '';
    if (left.length + right.length <= 50000 || typeof Worker === 'undefined') {
        return Promise.resolve(OSA.computeLineDiff(left, right));
    }
    return new Promise(function(resolve, reject) {
        const worker = OSA._ensureDiffWorker();
        if (!worker._routesJobs) {
            worker._routesJobs = true;
            worker.addEventListener('message', function(event) {
                const job = OSA._diffJobs.get(event.data.id);
                if (!job) return;
                OSA._diffJobs.delete(event.data.id);
                job.resolve(event.data);
            });
            worker.addEventListener('error', function(error) {
                OSA._diffJobs.forEach(job => job.reject(error));
                OSA._diffJobs.clear();
                worker.terminate(); OSA._diffWorker = null;
            });
        }
        const id = ++OSA._diffJobSequence;
        OSA._diffJobs.set(id, {resolve, reject});
        worker.postMessage({ id, oldText: left, newText: right });
    });
};
OSA.buildDiffHunks = function(lines, contextLines) {
    const context = typeof contextLines === 'number' ? contextLines : 3;
    const allLines = Array.isArray(lines) ? lines : [];
    const changed = [];

    allLines.forEach(function(line, index) {
        if (line && line.type !== 'ctx') changed.push(index);
    });

    if (!changed.length) {
        return [{
            oldStart: 1,
            oldCount: allLines.filter(function(line) { return line.oldNo !== null; }).length,
            newStart: 1,
            newCount: allLines.filter(function(line) { return line.newNo !== null; }).length,
            lines: allLines,
        }];
    }

    const ranges = [];
    changed.forEach(function(index) {
        const start = Math.max(0, index - context);
        const end = Math.min(allLines.length - 1, index + context);
        const prev = ranges[ranges.length - 1];
        if (prev && start <= prev.end + 1) {
            prev.end = Math.max(prev.end, end);
            return;
        }
        ranges.push({ start: start, end: end });
    });

    return ranges.map(function(range) {
        const hunkLines = allLines.slice(range.start, range.end + 1);
        const oldCount = hunkLines.filter(function(line) { return line.oldNo !== null; }).length;
        const newCount = hunkLines.filter(function(line) { return line.newNo !== null; }).length;

        let oldStart = 0;
        let newStart = 0;

        for (let i = 0; i < hunkLines.length; i += 1) {
            if (!oldStart && hunkLines[i].oldNo !== null) oldStart = hunkLines[i].oldNo;
            if (!newStart && hunkLines[i].newNo !== null) newStart = hunkLines[i].newNo;
            if (oldStart && newStart) break;
        }

        if (!oldStart && newStart) oldStart = Math.max(1, newStart);
        if (!newStart && oldStart) newStart = Math.max(1, oldStart);

        return {
            oldStart: oldStart || 1,
            oldCount,
            newStart: newStart || 1,
            newCount,
            lines: hunkLines,
        };
    });
};

OSA.formatDiffHunkHeader = function(hunk) {
    const oldStart = hunk && typeof hunk.oldStart === 'number' ? hunk.oldStart : 1;
    const oldCount = hunk && typeof hunk.oldCount === 'number' ? hunk.oldCount : 0;
    const newStart = hunk && typeof hunk.newStart === 'number' ? hunk.newStart : 1;
    const newCount = hunk && typeof hunk.newCount === 'number' ? hunk.newCount : 0;
    return '@@ -' + oldStart + ',' + oldCount + ' +' + newStart + ',' + newCount + ' @@';
};

OSA.renderDiffView = function(oldContent, newContent) {
    const root = document.createElement('div');
    root.className = 'diff-view';
    root.innerHTML = '<div class="diff-loading">Computing diff...</div>';

    OSA.computeLineDiffAsync(oldContent, newContent)
        .then(function(result) {
            const lines = Array.isArray(result.lines) ? result.lines : [];
            if (lines.length > 20000) {
                root.innerHTML = '<div class="diff-error">This diff exceeds the 20,000-line preview limit. Use Source to inspect the file.</div>';
                return;
            }
            const hunks = OSA.buildDiffHunks(lines, 3);
            const tableRows = hunks.map(function(hunk) {
                const headerRow = '<tr class="diff-hunk-header">'
                    + '<td colspan="3"><code>' + OSA.escapeHtml(OSA.formatDiffHunkHeader(hunk)) + '</code></td>'
                    + '</tr>';
                const lineRows = hunk.lines.map(function(line) {
                    const prefix = line.type === 'add' ? '+' : line.type === 'del' ? '-' : ' ';
                    const oldNo = line.oldNo === null ? '' : String(line.oldNo);
                    const newNo = line.newNo === null ? '' : String(line.newNo);
                    const location = oldNo || newNo ? oldNo + ':' + newNo : '';
                    return '<tr class="diff-line ' + line.type + '">'
                        + '<td class="diff-loc">' + OSA.escapeHtml(location) + '</td>'
                        + '<td class="diff-prefix">' + prefix + '</td>'
                        + '<td class="diff-text"><code>' + OSA.escapeHtml(line.text || '') + '</code></td>'
                        + '</tr>';
                }).join('');
                return headerRow + lineRows;
            }).join('');

            const sideRows = lines.map(function(line) {
                const leftText = line.type === 'add' ? '' : (line.text || '');
                const rightText = line.type === 'del' ? '' : (line.text || '');
                return '<tr class="diff-side-row">'
                    + '<td class="diff-side-cell old ' + line.type + '"><span class="diff-side-line-number">' + (line.oldNo ?? '') + '</span><code>' + OSA.escapeHtml(leftText) + '</code></td>'
                    + '<td class="diff-side-cell new ' + line.type + '"><span class="diff-side-line-number">' + (line.newNo ?? '') + '</span><code>' + OSA.escapeHtml(rightText) + '</code></td>'
                    + '</tr>';
            }).join('');

            root.innerHTML = ''
                + '<div class="diff-toolbar">'
                + '  <span class="diff-label">Unified diff</span>'
                + '  <button type="button" class="diff-toggle-btn" onclick="OSA.toggleDiffSideBySide(this)">Split view</button>'
                + '</div>'
                + '<table class="diff-table"><tbody>' + tableRows + '</tbody></table>'
                + '<div class="diff-side-by-side hidden">'
                + '  <table class="diff-side-table"><thead><tr><th>Before</th><th>After</th></tr></thead><tbody>' + sideRows + '</tbody></table>'
                + '</div>';
        })
        .catch(function(error) {
            root.innerHTML = '<div class="diff-error">Diff failed: ' + OSA.escapeHtml(error?.message || 'Unknown error') + '</div>';
        });

    return root;
};

OSA.toggleDiffSideBySide = function(button) {
    const root = button && button.closest ? button.closest('.diff-view') : null;
    if (!root) return;
    const side = root.querySelector('.diff-side-by-side');
    if (!side) return;
    const hidden = side.classList.toggle('hidden');
    root.querySelector('.diff-table')?.classList.toggle('hidden', !hidden);
    root.querySelector('.diff-label').textContent = hidden ? 'Unified diff' : 'Split diff';
    button.textContent = hidden ? 'Split view' : 'Unified view';
};
