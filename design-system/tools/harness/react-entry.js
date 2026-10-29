import * as React from 'react';
import * as ReactDOMClient from 'react-dom/client';
import * as ReactDOM from 'react-dom';
window.React = React;
window.ReactDOM = { ...ReactDOM, ...ReactDOMClient };
