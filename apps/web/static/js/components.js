class ShovelsHeader extends HTMLElement {
    connectedCallback() {
        const header = document.createElement('header');
        header.className = 'site-header';

        const brand = document.createElement('a');
        brand.href = '/';
        brand.className = 'site-brand';
        const logo = document.createElement('img');
        logo.src = '/static/logo.svg';
        logo.alt = 'ShovelsUp';
        brand.appendChild(logo);

        const nav = document.createElement('nav');
        nav.className = 'site-nav';

        for (const [label, href] of [
            [this.dataset.permits || 'Permits', '/permits'],
            [this.dataset.council || 'Council', '/council'],
        ]) {
            const a = document.createElement('a');
            a.href = href;
            a.textContent = label;
            nav.appendChild(a);
        }

        header.appendChild(brand);
        header.appendChild(nav);
        this.appendChild(header);
    }
}

customElements.define('shovels-header', ShovelsHeader);
