// Use the URL: http://127.0.0.1:8765/check?id={{test_id}}
pm.variables.set('test_id', '123');
pm.request.method = 'POST';
pm.request.headers.upsert({ key: 'Content-Type', value: 'application/json' });
pm.request.headers.upsert({ key: 'X-Script-Check', value: 'pre-ok' });
pm.request.body.update(JSON.stringify({ message: 'hello from pre-script' }));

pm.test('Pre-request: local variable is set', () => {
  pm.expect(pm.variables.get('test_id')).to.equal('123');
});
console.log('Pre-request script executed', pm.variables.get('test_id'));
