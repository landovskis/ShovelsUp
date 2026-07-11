// IMP-REQ-009-07: wires the Confirm/Reject buttons on the review-queue
// page to the JSON routes, and shows the stale-conflict banner on a 409
// without a full page reload.
//
// Known limitation (inherited, not introduced here): `middleware::admin_auth`
// returns 403 with no `WWW-Authenticate` challenge, so browsers never show
// a native Basic Auth login prompt for this page — it was originally built
// for programmatic clients (curl/k6), not interactive browser sessions.
// These fetch() calls rely on the browser already holding credentials for
// this origin (e.g. injected by a reverse proxy or browser extension);
// there is no in-app login flow. See docs/runbooks/review_queue.md.
document.addEventListener('DOMContentLoaded', () => {
  const banner = document.getElementById('stale-conflict-banner');
  const section = document.querySelector('.review-queue');
  const confirmLabel = section ? section.dataset.confirmLabel : 'Confirm';
  const rejectLabel = section ? section.dataset.rejectLabel : 'Reject';

  document.querySelectorAll('.candidate-actions').forEach((form) => {
    const candidateId = form.dataset.candidateId;
    const version = parseInt(form.dataset.version, 10);

    form.querySelectorAll('button[data-action]').forEach((button) => {
      button.addEventListener('click', async () => {
        const action = button.dataset.action;
        const url = `/admin/review_candidates/${candidateId}/${action}`;
        const body = { version };
        if (action === 'confirm') {
          const projectIdInput = form.querySelector('.candidate-project-id');
          body.project_id = projectIdInput ? projectIdInput.value : '';
        }

        button.disabled = true;
        const originalLabel = button.textContent;
        button.textContent = action === 'confirm' ? `${confirmLabel}…` : `${rejectLabel}…`;

        try {
          const response = await fetch(url, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(body),
          });

          if (response.status === 409) {
            if (banner) {
              banner.hidden = false;
            }
            button.disabled = false;
            button.textContent = originalLabel;
            return;
          }

          if (!response.ok) {
            button.disabled = false;
            button.textContent = originalLabel;
            return;
          }

          // Success: this candidate has left the current tab (open ->
          // confirmed/rejected) — remove it from the list rather than
          // reloading the whole page.
          const item = form.closest('.review-candidate');
          if (item) {
            item.remove();
          }
        } catch (err) {
          button.disabled = false;
          button.textContent = originalLabel;
        }
      });
    });
  });
});
