self.onmessage = function(event) {
    const payload = event.data || {};
    const oldText = typeof payload.oldText === 'string' ? payload.oldText : '';
    const newText = typeof payload.newText === 'string' ? payload.newText : '';

    const oldLines = oldText.split('\n');
    const newLines = newText.split('\n');
    const m = oldLines.length;
    const n = newLines.length;
    const dp = Array.from({ length: m + 1 }, function() {
        return new Array(n + 1).fill(0);
    });

    for (let i = m - 1; i >= 0; i -= 1) {
        for (let j = n - 1; j >= 0; j -= 1) {
            if (oldLines[i] === newLines[j]) {
                dp[i][j] = dp[i + 1][j + 1] + 1;
            } else {
                dp[i][j] = Math.max(dp[i + 1][j], dp[i][j + 1]);
            }
        }
    }

    const lines = [];
    let i = 0;
    let j = 0;
    let oldNo = 1;
    let newNo = 1;

    while (i < m && j < n) {
        if (oldLines[i] === newLines[j]) {
            lines.push({ type: 'ctx', text: oldLines[i], oldNo: oldNo, newNo: newNo });
            i += 1;
            j += 1;
            oldNo += 1;
            newNo += 1;
        } else if (dp[i + 1][j] >= dp[i][j + 1]) {
            lines.push({ type: 'del', text: oldLines[i], oldNo: oldNo, newNo: null });
            i += 1;
            oldNo += 1;
        } else {
            lines.push({ type: 'add', text: newLines[j], oldNo: null, newNo: newNo });
            j += 1;
            newNo += 1;
        }
    }

    while (i < m) {
        lines.push({ type: 'del', text: oldLines[i], oldNo: oldNo, newNo: null });
        i += 1;
        oldNo += 1;
    }

    while (j < n) {
        lines.push({ type: 'add', text: newLines[j], oldNo: null, newNo: newNo });
        j += 1;
        newNo += 1;
    }

    self.postMessage({ lines: lines });
};
