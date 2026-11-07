# agents-tmux website

Documentation site for [agents-tmux](https://github.com/ahmad9059/AgentsTmux), built with [Astro Starlight](https://starlight.astro.build/).

## Local development

```sh
npm install
npm run dev
```

Open <http://localhost:4321/AgentsTmux/> to view.

## Build

```sh
npm run build     # generates dist/
npm run preview   # serves built site locally
```

Deployed to GitHub Pages by `.github/workflows/deploy-website.yml` on every push to `main` that touches `website/**` or the workflow file itself.
