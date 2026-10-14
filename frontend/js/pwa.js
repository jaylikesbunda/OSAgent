// Progressive web app glue: registers the service worker so iOS and Android
// can install the web UI as a standalone app with the OSA icon.
window.OSA = window.OSA || {};

OSA.registerServiceWorker = function() {
    if (!('serviceWorker' in navigator)) return;
    // Service workers require a secure context: https (the cloudflared tunnel)
    // or localhost. Plain http on the LAN does not qualify, and registration
    // simply does not happen there.
    if (!window.isSecureContext) return;

    const register = () => {
        navigator.serviceWorker.register('/sw.js').catch(function(error) {
            console.debug('Service worker registration failed:', error);
        });
    };
    if (document.readyState === 'complete') {
        register();
    } else {
        window.addEventListener('load', register, { once: true });
    }
};

OSA.registerServiceWorker();
