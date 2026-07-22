// IMP-REQ-011-04: progressive-enhancement behavior for the mobile filter
// sheet on `search.html`. With this script absent/failing, the filter form
// is still present and submittable in the DOM (no JS-required functionality
// is lost) — this only adds the open/close toggle used at narrow viewports,
// where `main.css` collapses `#filter-sheet` behind `#filter-sheet-trigger`.
//
// Deliberately not a `<dialog>`/modal: no backdrop, no scroll lock, no focus
// trap. Opening the sheet does not disable interaction with the rest of the
// page (TC-011-3's "non-blocking" requirement) — it is simply revealed via
// the `.filter-sheet-open` class. Closing it returns keyboard focus to the
// trigger button that opened it.
const trigger = document.getElementById('filter-sheet-trigger');
const sheet = document.getElementById('filter-sheet');
const closeButton = document.getElementById('filter-sheet-close');

if (trigger && sheet && closeButton) {
    trigger.addEventListener('click', () => {
        sheet.classList.add('filter-sheet-open');
        trigger.setAttribute('aria-expanded', 'true');
        closeButton.focus();
    });

    closeButton.addEventListener('click', () => {
        sheet.classList.remove('filter-sheet-open');
        trigger.setAttribute('aria-expanded', 'false');
        trigger.focus();
    });
}
