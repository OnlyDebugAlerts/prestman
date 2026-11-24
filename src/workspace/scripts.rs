use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ScriptPhase {
    Pre,
    OnMessage,
    Post,
}

impl Workspace {
    pub(super) fn insert_script_example(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (editor, example) = match self.script_phase {
            ScriptPhase::OnMessage => (
                &self.on_message_script,
                "pm.test('Received a message', () => {\n  pm.expect(pm.message.data).to.be.an('object');\n});\n",
            ),
            ScriptPhase::Pre if self.active().draft.grpc.is_some() => (
                &self.pre_script,
                "pm.variables.set('timestamp', String(Date.now()));\nconsole.log('Invoking', pm.request.methodPath);\n",
            ),
            ScriptPhase::Post if self.active().draft.grpc.is_some() => (
                &self.post_script,
                "pm.test('gRPC status is OK', () => {\n  pm.expect(pm.response.statusCode).to.equal(0);\n});\nconsole.log('Response messages:', pm.response.messages.count());\n",
            ),
            ScriptPhase::Pre => (
                &self.pre_script,
                "// Use {{timestamp}} in the URL, headers or body.\npm.variables.set('timestamp', String(Date.now()));\npm.request.headers.upsert({ key: 'X-Request-Time', value: '{{timestamp}}' });\nconsole.log('Preparing', pm.request.method, String(pm.request.url));\n",
            ),
            ScriptPhase::Post => (
                &self.post_script,
                "pm.test('Status is 200', () => {\n  pm.response.to.have.status(200);\n});\n\npm.test('Response is JSON', () => {\n  pm.expect(pm.response.json()).to.be.an('object');\n});\n\n// Save a value for subsequent requests:\n// pm.environment.set('token', pm.response.json().token);\nconsole.log('Response time:', pm.response.responseTime, 'ms');\n",
            ),
        };
        if editor.read(cx).value().is_empty() {
            editor.update(cx, |editor, cx| editor.replace_all(example, window, cx));
            editor.read(cx).focus_handle(cx).focus(window, cx);
        }
    }

    pub(super) fn apply_script_mutations(
        &mut self,
        collection_id: Option<usize>,
        mutations: &[scripting::Mutation],
        cx: &mut Context<Self>,
    ) {
        if mutations.is_empty() {
            return;
        }
        for mutation in mutations {
            match mutation.scope.as_str() {
                "globals" => mutation.apply(&mut self.global_variables),
                "environment" => mutation.apply(&mut self.environment_variables),
                "collection" if self.in_team() => {
                    self.set_team_collection_value(collection_id, mutation)
                }
                "collection" => {
                    if let Some(collection) = self
                        .collections
                        .iter_mut()
                        .find(|item| Some(item.id) == collection_id)
                    {
                        mutation.apply(&mut collection.variables);
                    }
                }
                _ => {}
            }
        }
        if self.in_team() {
            if let Err(error) = self.save_team_cache(false, None) {
                self.save_error = Some(format!("Could not save script variables. {error:#}"));
            }
        } else {
            self.persist("Could not save script variables", cx);
        }
    }
}
