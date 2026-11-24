use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::Result;
use prost_reflect::DescriptorPool;
use prost_types::{DescriptorProto, FileDescriptorProto};

pub(super) fn build_pool(
    mut files: BTreeMap<String, FileDescriptorProto>,
) -> Result<DescriptorPool> {
    let mut pool = DescriptorPool::new();
    let Err(error) = pool.add_file_descriptor_protos(files.values().cloned()) else {
        return Ok(pool);
    };
    if !restore_imports(&mut files) {
        return Err(schema_error(error));
    }

    let mut pool = DescriptorPool::new();
    pool.add_file_descriptor_protos(files.into_values())
        .map_err(schema_error)?;
    Ok(pool)
}

fn schema_error(error: prost_reflect::DescriptorError) -> anyhow::Error {
    let message = error.file().map_or_else(
        || "Server reflection returned an invalid Protobuf schema".to_owned(),
        |file| format!("Server reflection returned an invalid Protobuf schema in {file}"),
    );
    anyhow::Error::new(error).context(message)
}

// Some reflection implementations return the type definitions but omit their import lists.
fn restore_imports(files: &mut BTreeMap<String, FileDescriptorProto>) -> bool {
    let mut symbols = HashMap::new();
    for (name, file) in files.iter() {
        for enumeration in &file.enum_type {
            symbols.insert(qualify(file.package(), enumeration.name()), name.as_str());
        }
        visit_messages(file.package(), &file.message_type, &mut |scope, message| {
            symbols.insert(scope.to_owned(), name.as_str());
            for enumeration in &message.enum_type {
                symbols.insert(qualify(scope, enumeration.name()), name.as_str());
            }
        });
    }

    let imports = files
        .iter()
        .map(|(name, file)| {
            let mut imports = BTreeSet::new();
            let mut add_import = |scope: &str, reference: &str| {
                if !reference.is_empty()
                    && let Some(owner) = defining_file(&symbols, scope, reference)
                    && owner != name
                    && !file.dependency.iter().any(|dependency| dependency == owner)
                {
                    imports.insert(owner.to_owned());
                }
            };
            for service in &file.service {
                let scope = qualify(file.package(), service.name());
                for method in &service.method {
                    add_import(&scope, method.input_type());
                    add_import(&scope, method.output_type());
                }
            }
            for field in &file.extension {
                add_import(file.package(), field.extendee());
                add_import(file.package(), field.type_name());
            }
            visit_messages(file.package(), &file.message_type, &mut |scope, message| {
                for field in message.field.iter().chain(&message.extension) {
                    add_import(scope, field.extendee());
                    add_import(scope, field.type_name());
                }
            });
            imports
        })
        .collect::<Vec<_>>();

    let changed = imports.iter().any(|dependencies| !dependencies.is_empty());
    for (file, dependencies) in files.values_mut().zip(imports) {
        // Append so indices in public_dependency and weak_dependency stay valid.
        file.dependency.extend(dependencies);
    }
    changed
}

fn visit_messages(
    scope: &str,
    messages: &[DescriptorProto],
    visit: &mut impl FnMut(&str, &DescriptorProto),
) {
    for message in messages {
        let scope = qualify(scope, message.name());
        visit(&scope, message);
        visit_messages(&scope, &message.nested_type, visit);
    }
}

fn defining_file<'a>(
    symbols: &HashMap<String, &'a str>,
    mut scope: &str,
    reference: &str,
) -> Option<&'a str> {
    if let Some(absolute) = reference.strip_prefix('.') {
        return symbols.get(absolute).copied();
    }
    while !scope.is_empty() {
        if let Some(file) = symbols.get(&qualify(scope, reference)) {
            return Some(*file);
        }
        scope = scope.rsplit_once('.').map_or("", |(parent, _)| parent);
    }
    symbols.get(reference).copied()
}

fn qualify(scope: &str, name: &str) -> String {
    if scope.is_empty() {
        name.to_owned()
    } else {
        format!("{scope}.{name}")
    }
}
