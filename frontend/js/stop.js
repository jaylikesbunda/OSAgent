window.OSA = window.OSA || {};

OSA._forceResetState = function(sessionId) {
    if (sessionId && OSA.getCurrentSession()?.id !== sessionId) return;
    OSA.setProcessing(false);
    OSA.setStopping(false);
    OSA.resetSendButton();
    OSA.hideThinkingIndicator();
    OSA.pruneEmptyStreamingMessage();
    OSA.completeAssistantResponse();
    OSA.tmodelSettleRunningItems?.('cancelled');
    if (OSA._stopTimeout) {
        clearTimeout(OSA._stopTimeout);
        OSA._stopTimeout = null;
    }
};

OSA.stopGeneration = async function() {
    const session = OSA.getCurrentSession();
    if (!session?.id || OSA.isAgentStopping()) return;
    const sessionId = session.id;
    OSA.setStopping(true);
    OSA.cancelSpeechOutput?.();
    const button = document.getElementById('send-btn');
    if (button) {
        button.disabled = true;
        button.setAttribute('aria-label', 'Stopping');
        button.title = 'Stopping…';
    }
    if (OSA._stopTimeout) clearTimeout(OSA._stopTimeout);
    OSA._stopTimeout = setTimeout(async () => {
        OSA._stopTimeout = null;
        if (OSA.getCurrentSession()?.id !== sessionId || !OSA.isAgentStopping()) return;
        try {
            const response = await OSA.fetchWithAuth(`/api/sessions/${encodeURIComponent(sessionId)}`);
            if (!response.ok) throw new Error('Could not confirm cancellation.');
            const snapshot = await response.json();
            if (OSA.getCurrentSession()?.id !== sessionId || !OSA.isAgentStopping()) return;
            if (snapshot.task_status !== 'running') {
                OSA.handleEventCancelled?.({ session_id: sessionId });
                return;
            }
            throw new Error('Still stopping. You can retry Stop.');
        } catch (error) {
            if (OSA.getCurrentSession()?.id !== sessionId || !OSA.isAgentStopping()) return;
            OSA.setStopping(false);
            OSA.setSendButtonStopMode(true);
            OSA.showToast?.(error.message);
        }
    }, 5000);
    try {
        await OSA.cancelSession(sessionId);
    } catch (error) {
        if (OSA.getCurrentSession()?.id !== sessionId || !OSA.isAgentStopping()) return;
        if (OSA._stopTimeout) clearTimeout(OSA._stopTimeout);
        OSA._stopTimeout = null;
        OSA.setStopping(false);
        OSA.setSendButtonStopMode(true);
        OSA.showToast?.(error.message || 'Stop failed. Try again.');
    }
};
