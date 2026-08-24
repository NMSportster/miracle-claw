// Tauri shim for dev-only Vite testing. Returns real-looking data
// so boot() can complete without Tauri runtime.
(function () {
  const mockReport = {
    needs_maic_login: false,
    user: { id: 1, email: 'dev@local', tier: 'pro' },
    nudges: [],
  };
  function fakeInvoke(cmd, args) {
    if (cmd === 'first_run_report') return Promise.resolve(mockReport);
    if (cmd === 'mc_get_user_info') return Promise.resolve({ tier: 'pro', email: 'dev@local' });
    if (cmd === 'mc_list_tools') return Promise.resolve([]);
    return Promise.resolve(null);
  }
  window.__TAURI_INTERNALS__ = {
    invoke: fakeInvoke,
    metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main', windowLabel: 'main' } },
  };
  window.__TAURI__ = {
    core: { invoke: fakeInvoke },
    event: {
      listen: function () { return Promise.resolve(function () {}); },
      emit: function () { return Promise.resolve(); },
    },
  };
})();
