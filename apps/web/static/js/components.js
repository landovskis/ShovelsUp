class ShovelsHeader extends HTMLElement {
    connectedCallback() {
        const header = document.createElement('header');
        header.style.cssText = 'display:flex;align-items:center;justify-content:space-between;padding:1rem 1.5rem;background:#fff;border-bottom:1px solid #e7e5e4;';

        const brand = document.createElement('a');
        brand.href = '/';
        brand.textContent = 'ShovelsUp';
        brand.style.cssText = 'font-weight:700;font-size:1.25rem;color:#f97316;text-decoration:none;';

        const nav = document.createElement('nav');
        nav.style.cssText = 'display:flex;gap:1.5rem;';

        for (const [label, href] of [['Permits', '/permits'], ['Council', '/council']]) {
            const a = document.createElement('a');
            a.href = href;
            a.textContent = label;
            a.style.cssText = 'color:#1c1917;text-decoration:none;';
            nav.appendChild(a);
        }

        header.appendChild(brand);
        header.appendChild(nav);
        this.appendChild(header);
    }
}

customElements.define('shovels-header', ShovelsHeader);
