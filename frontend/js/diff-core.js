(function(scope) {
    scope.OSADiff = {
        compute(oldText, newText) {
            const a = String(oldText ?? '').split('\n');
            const b = String(newText ?? '').split('\n');
            let prefix = 0, suffix = 0;
            while (prefix < a.length && prefix < b.length && a[prefix] === b[prefix]) prefix++;
            while (suffix < a.length - prefix && suffix < b.length - prefix && a[a.length - 1 - suffix] === b[b.length - 1 - suffix]) suffix++;
            const m = a.length - prefix - suffix, n = b.length - prefix - suffix;
            const lines = [];
            let oldNo = 1, newNo = 1;
            const add = (type, text) => lines.push({ type, text, oldNo: type === 'add' ? null : oldNo++, newNo: type === 'del' ? null : newNo++ });
            for (let k = 0; k < prefix; k++) add('ctx', a[k]);
            // Bound memory and computation; huge replacements remain a valid,
            // non-minimal diff rather than allocating a quadratic table.
            if ((m + 1) * (n + 1) > 1000000) {
                for (let i = 0; i < m; i++) add('del', a[prefix + i]);
                for (let j = 0; j < n; j++) add('add', b[prefix + j]);
            } else {
                const width = n + 1;
                const dp = new Uint32Array((m + 1) * width);
                for (let i = m - 1; i >= 0; i--) for (let j = n - 1; j >= 0; j--)
                    dp[i * width + j] = a[prefix + i] === b[prefix + j]
                        ? dp[(i + 1) * width + j + 1] + 1
                        : Math.max(dp[(i + 1) * width + j], dp[i * width + j + 1]);
                let i = 0, j = 0;
                while (i < m && j < n) {
                    if (a[prefix + i] === b[prefix + j]) { add('ctx', a[prefix + i]); i++; j++; }
                    else if (dp[(i + 1) * width + j] >= dp[i * width + j + 1]) add('del', a[prefix + i++]);
                    else add('add', b[prefix + j++]);
                }
                while (i < m) add('del', a[prefix + i++]);
                while (j < n) add('add', b[prefix + j++]);
            }
            for (let k = suffix; k > 0; k--) add('ctx', a[a.length - k]);
            return { lines };
        }
    };
})(typeof window === 'undefined' ? self : window);
