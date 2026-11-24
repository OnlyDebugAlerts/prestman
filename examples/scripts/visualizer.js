// Use with the local server from examples/scripts/README.md and pre-request.js.
const template = `
<style>
  body { font-family: system-ui; background: #faf9ff; color: #24202f; }
  h1 { font-size: 20px; }
  table { border-collapse: collapse; width: 100%; margin: 16px 0; }
  th, td { padding: 9px; border-bottom: 1px solid #ded9eb; text-align: left; }
  th { background: #eee9fa; }
  button { padding: 8px 12px; border: 0; border-radius: 6px; background: #704ad6; color: white; cursor: pointer; }
  pre { padding: 12px; background: white; border-radius: 6px; overflow: auto; }
</style>
<h1>{{response.method}} · request {{response.query.id}}</h1>
<p>Status: {{status}} · {{elapsed}} ms</p>
<table>
  <tr><th>Header</th><th>Value</th></tr>
  {{#each response.headers}}
  <tr><td>{{@key}}</td><td>{{this}}</td></tr>
  {{/each}}
</table>
<button id="toggle-body">Show / hide request body</button>
<pre id="body"></pre>
<p>Header lengths (canvas):</p>
<canvas id="chart" width="500" height="220"></canvas>
<script>
pm.getData(function (error, data) {
  const output = document.getElementById('body');
  if (error) { output.textContent = String(error); return; }
  output.textContent = JSON.stringify(data.response.body, null, 2);
  document.getElementById('toggle-body').onclick = () => { output.hidden = !output.hidden; };
  const context = document.getElementById('chart').getContext('2d');
  context.font = '12px system-ui';
  Object.entries(data.response.headers).slice(0, 7).forEach(([name, value], index) => {
    context.fillStyle = '#24202f';
    context.fillText(name, 0, index * 28 + 15);
    context.fillStyle = '#9274db';
    context.fillRect(160, index * 28, Math.min(String(value).length * 5, 330), 20);
  });
});
</script>
`;

pm.visualizer.set(template, {
  response: pm.response.json(),
  status: pm.response.code,
  elapsed: pm.response.responseTime
});
