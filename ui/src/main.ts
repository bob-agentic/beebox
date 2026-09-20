import { mount } from 'svelte';
import App from './App.svelte';
import './app.css';
import '@xterm/xterm/css/xterm.css';

if (!('__BEEBOX__' in window)) document.body.classList.add('web');

mount(App, { target: document.getElementById('app')! });
