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

        const projectsLink = document.createElement('a');
        projectsLink.href = '/projects';
        projectsLink.textContent = this.dataset.projects || 'Projects';
        nav.appendChild(projectsLink);

        header.appendChild(brand);
        header.appendChild(nav);
        this.appendChild(header);
    }
}

customElements.define('shovels-header', ShovelsHeader);
