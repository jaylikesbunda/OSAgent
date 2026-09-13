const assert = require('node:assert/strict');
const test = require('node:test');

global.window = global;
require('../js/tools.js');

test('context meter switches from estimate to provider usage', () => {
    const estimated = OSA.getContextRingMetrics({
        estimated_tokens: 400,
        context_window: 1000,
    });
    assert.equal(estimated.used, 400);
    assert.equal(estimated.estimated, true);
    assert.match(OSA.buildContextRingHtml({ estimated_tokens: 400, context_window: 1000 }, 'x'), /estimate/);
    assert.match(OSA.buildContextRingHtml({ estimated_tokens: 400, context_window: 1000 }, 'x'), />40%~</);

    const reported = OSA.getContextRingMetrics({
        estimated_tokens: 400,
        context_window: 1000,
        last_request_usage: { input: 100, cached_read: 50 },
    });
    assert.equal(reported.used, 150);
    assert.equal(reported.estimated, false);
    assert.match(OSA.buildContextRingHtml({
        estimated_tokens: 400,
        context_window: 1000,
        last_request_usage: { input: 100, cached_read: 50 },
    }, 'x'), /provider-reported/);
});

test('response usage updates the current session meter', () => {
    OSA._currentContextSessionId = 'session-1';
    OSA._contextStates = {
        'session-1': { estimated_tokens: 400, context_window: 1000 },
    };
    let updated = null;
    const originalUpdate = OSA.updateContextStatus;
    OSA.updateContextStatus = state => { updated = state; };

    OSA.updateContextFromResponseUsage({ input: 120, cached_read: 30, total: 120 });

    assert.deepEqual(updated.last_request_usage, {
        input: 120,
        cached_read: 30,
        total: 120,
    });
    assert.equal(OSA.getContextRingMetrics(updated).used, 150);
    assert.equal(OSA.getContextRingMetrics(updated).estimated, false);
    OSA.updateContextStatus = originalUpdate;
});
