import { mount } from 'svelte';
import App from './App.svelte';
import './app.css';
import { applyTheme, cachedTheme } from './lib/theme';
import { installTooltips } from './lib/tooltip';

applyTheme(cachedTheme());
installTooltips();

mount(App, { target: document.getElementById('app')! });
