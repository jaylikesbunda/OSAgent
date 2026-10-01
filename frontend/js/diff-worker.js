importScripts('/static/js/diff-core.js');
self.onmessage = function(event) {
    const payload = event.data || {};
    self.postMessage({ id: payload.id, ...self.OSADiff.compute(payload.oldText, payload.newText) });
};