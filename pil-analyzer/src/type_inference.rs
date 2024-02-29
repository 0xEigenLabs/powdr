use std::collections::{BTreeSet, HashMap};

use itertools::Itertools;
use powdr_ast::{
    analyzed::{
        types::{
            format_type_scheme_around_name, ArrayType, FunctionType, TupleType, Type, TypeScheme,
        },
        Expression, PolynomialReference, Reference,
    },
    parsed::{
        visitor::ExpressionVisitable, ArrayLiteral, FunctionCall, IndexAccess, LambdaExpression,
        MatchArm, MatchPattern, TypeBounds, TypeName,
    },
};
use powdr_number::FieldElement;

use crate::{
    call_graph::sort_called_first,
    type_builtins::{
        binary_operator_scheme, builtin_schemes, type_for_reference, unary_operator_scheme,
    },
    type_unifier::Unifier,
};

// TODO we should check at the end that the literals are not too large for the inferred types.

/// Infers types on all definitions and checks type-correctness for isolated
/// expressions (from identities and arrays) where the expected type is given.
/// Sets the generic arguments for references and the literal types in all expressions.
/// Returns the types for symbols without explicit type.
pub fn infer_types<T: FieldElement>(
    definitions: HashMap<String, (Option<TypeScheme>, Option<&mut Expression<T>>)>,
    expressions: &mut [(&mut Expression<T>, ExpectedType)],
) -> Result<Vec<(String, Type)>, String> {
    TypeChecker::default().infer_types(definitions, expressions)
}

/// A type to expect and a flag that says if arrays of that type are also fine.
#[derive(Clone)]
pub struct ExpectedType {
    pub ty: Type,
    pub allow_array: bool,
}

impl From<Type> for ExpectedType {
    fn from(ty: Type) -> Self {
        ExpectedType {
            ty,
            allow_array: false,
        }
    }
}

#[derive(Default)]
struct TypeChecker {
    /// Types for local variables, might contain type variables.
    local_var_types: Vec<Type>,
    /// Declared types for symbols. Type scheme for polymorphic symbols
    /// and unquantified type variables for symbols without type.
    declared_types: HashMap<String, TypeScheme>,
    /// These are the inferred types for symbols that are declared
    /// as type schemes. They are compared to the declared types
    /// at the very end.
    inferred_types: HashMap<String, Type>,
    unifier: Unifier,
    last_type_var: usize,
}

impl TypeChecker {
    pub fn infer_types<T: FieldElement>(
        mut self,
        mut definitions: HashMap<String, (Option<TypeScheme>, Option<&mut Expression<T>>)>,
        expressions: &mut [(&mut Expression<T>, ExpectedType)],
    ) -> Result<Vec<(String, Type)>, String> {
        let type_var_mapping = self.infer_types_inner(&mut definitions, expressions)?;
        self.update_generic_args(&mut definitions, expressions, &type_var_mapping)?;
        Ok(definitions
            .into_iter()
            .filter(|(_, (ty, _))| ty.is_none())
            .map(|(name, _)| {
                let mut scheme = self.declared_types[&name].clone();
                assert!(scheme.vars.is_empty());
                self.substitute(&mut scheme.ty);
                assert!(scheme.ty.is_concrete_type());
                (name, scheme.ty)
            })
            .collect())
    }

    /// Returns, for each name declared with a type scheme, a mapping from
    /// the type variables used by the type checker to those used in the declaration.
    fn infer_types_inner<T: FieldElement>(
        &mut self,
        definitions: &mut HashMap<String, (Option<TypeScheme>, Option<&mut Expression<T>>)>,
        expressions: &mut [(&mut Expression<T>, ExpectedType)],
    ) -> Result<HashMap<String, HashMap<String, Type>>, String> {
        // TODO in order to fix type inference on recursive functions, we need to:
        // - collect all groups of functions that call each other recursively
        // - analyze each such group in an environment, where their type schemes
        //   are instantiated once at the start and not anymore for the symbol lookup.

        // Sort the names such that called names occur first.
        let names = sort_called_first(
            definitions
                .iter()
                .map(|(n, (_, v))| (n.as_str(), v.as_deref())),
        );

        // Remove builtins from definitions and check they types are correct.
        for (name, ty) in builtin_schemes() {
            if let Some((_, (Some(defined_ty), _))) = definitions.remove_entry(name) {
                assert!(
                    ty == &defined_ty,
                    "Invalid type for built-in scheme {name}: {}",
                    format_type_scheme_around_name(name, &Some(defined_ty))
                );
            }
        }

        self.declared_types = builtin_schemes().clone();
        // Add types from declarations. Type schemes are added without instantiating.
        for (name, def) in definitions.iter() {
            // This stores an (uninstantiated) type scheme for symbols with a declared
            // polymorphic type and it creates a new (unquantified) type variable for
            // symbols without declared type. This forces a single concrete type for the latter.
            let ty = def.0.clone().unwrap_or_else(|| self.new_type_var().into());
            self.declared_types.insert(name.clone(), ty);
        }

        // Now go through all symbols and derive types for the expressions.
        // While analyzing a symbol, we ignore its declared type (unless the
        // symbol is referenced). Unifying the declared type with the inferred
        // type is done at the end.
        for name in names {
            // Ignore builtins (removed from definitions) and definitions without value.
            let Some((_, Some(value))) = definitions.get_mut(&name) else {
                continue;
            };

            let declared_type = self.declared_types[&name].clone();
            (if declared_type.vars.is_empty() {
                match &declared_type.ty {
                    Type::Col => {
                        // This is a column. It means we prefer `int -> fe`, but `int -> int`
                        // is also OK if it can be derived directly.
                        let return_type = self.new_type_var();
                        let fun_type = Type::Function(FunctionType {
                            params: vec![Type::Int],
                            value: Box::new(return_type.clone()),
                        });
                        self.expect_type(&fun_type, value)?;
                        match self.substitute_to(return_type) {
                            Type::Int => Ok(()),
                            t => self
                                .unifier
                                .unify_types(t.clone(), Type::Fe)
                                .map_err(|err| {
                                    format!(
                                        "Return type is expected to be `fe`, but got:{}.\n{err}",
                                        self.substitute_to(t)
                                    )
                                }),
                        }
                    }
                    Type::Array(ArrayType { base, length: _ }) if base.as_ref() == &Type::Col => {
                        // An array of columns. We prefer `(int -> fe)[]`, but we also allow `(int -> int)[]`.
                        // Also we ignore the length.
                        let return_type = self.new_type_var();
                        let fun_type = Type::Function(FunctionType {
                            params: vec![Type::Int],
                            value: Box::new(return_type.clone()),
                        });
                        let arr = Type::Array(ArrayType {
                            base: fun_type.into(),
                            length: None,
                        });
                        self.expect_type(&arr, value)?;
                        match self.substitute_to(return_type) {
                            Type::Int => Ok(()),
                            t => self
                                .unifier
                                .unify_types(t.clone(), Type::Fe)
                                .map_err(|err| {
                                    format!(
                                        "Return type is expected to be `fe`, but got:{}.\n{err}",
                                        self.substitute_to(t)
                                    )
                                }),
                        }
                    }
                    Type::Array(ArrayType {
                        base,
                        length: Some(_),
                    }) if base.as_ref() == &Type::Expr => {
                        // An array of intermediate columns with fixed length. We ignore the length.
                        // The condenser will have to check the actual length.
                        let arr = Type::Array(ArrayType {
                            base: base.clone(),
                            length: None,
                        });
                        self.expect_type(&arr, value).map_err(|e| {
                            format!("Expected dynamically-sized array for symbol {name}:\n{e}")
                        })
                    }
                    t => self.expect_type(t, value),
                }
            } else {
                self.process_expression(value).map(|ty| {
                    self.inferred_types.insert(name.to_string(), ty);
                })
            })
            .map_err(|e| format!("Error type checking the symbol {name} = {value}:\n{e}",))?;
        }

        self.check_expressions(expressions)?;

        // From this point on, the substitutions are fixed.

        // Now we check for all symbols that are not declared as a type scheme that they
        // can resolve to a concrete type.
        for (name, declared_type) in &self.declared_types {
            if declared_type.vars.is_empty() {
                // It is not a type scheme, see if we were able to derive a concrete type.
                let inferred = self.substitute_to(declared_type.ty.clone());
                if !inferred.is_concrete_type() {
                    let inferred_scheme = self.to_type_scheme(inferred);
                    Err(format!(
                        "Could not derive a concrete type for symbol {name}.\nInferred type scheme: {}\n",
                        format_type_scheme_around_name(
                            name,
                            &Some(inferred_scheme),
                        )
                    ))?;
                }
            }
        }

        // We have to check type schemes last, because only at this point do we know
        // that other types that should be concrete do not occur as type variables in the
        // inferred type scheme any more.
        let type_var_mapping = self.inferred_types.iter().map(|(name, inferred_type)| {
            let inferred = self.to_type_scheme(inferred_type.clone());
            let declared = self.declared_types[name].clone().simplify_type_vars();
            if inferred != declared {
                Err(format!(
                        "Inferred type scheme for symbol {name} does not match the declared type.\nInferred: let{} {name}: {}\nDeclared: let{} {name}: {}",
                        inferred.type_vars_to_string(),
                        inferred.ty,
                        self.declared_types[name].type_vars_to_string(),
                        self.declared_types[name].ty
                    ))?;
            }
            let declared_type_vars = self.declared_types[name].ty.contained_type_vars();
            let inferred_type = self.substitute_to(inferred_type.clone());
            let inferred_type_vars = inferred_type.contained_type_vars();
            Ok((name.clone(),
                inferred_type_vars
                    .into_iter()
                    .cloned()
                    .zip(declared_type_vars.into_iter().map(|tv| Type::TypeVar(tv.clone())))
                    .collect(),
            ))
        }).collect::<Result<_, String>>()?;

        Ok(type_var_mapping)
    }

    /// Updates generic arguments and literal annotations with the proper resolved types.
    /// `type_var_mapping` is a mapping (for each generic symbol) from
    /// the type variable names used by the type checker to those from the declaration.
    fn update_generic_args<T: FieldElement>(
        &mut self,
        definitions: &mut HashMap<String, (Option<TypeScheme>, Option<&mut Expression<T>>)>,
        expressions: &mut [(&mut Expression<T>, ExpectedType)],
        type_var_mapping: &HashMap<String, HashMap<String, Type>>,
    ) -> Result<(), String> {
        let mut errors = vec![];
        definitions
            .iter_mut()
            .filter_map(|(name, (_, expr))| expr.as_mut().map(|expr| (name, expr)))
            .for_each(|(name, expr)| {
                let empty_mapping = Default::default();
                let var_mapping = type_var_mapping.get(name).unwrap_or(&empty_mapping);
                expr.post_visit_expressions_mut(&mut |e| {
                    if let Err(e) = self.update_generic_args_for_expression(e, var_mapping) {
                        // TODO cannot borrow the value here for printing it.
                        // We should fix this properly by using source references.
                        errors.push(format!(
                            "Error specializing generic references in {name}:\n{e}",
                        ))
                    }
                });
            });

        for (expr, _) in expressions {
            expr.post_visit_expressions_mut(&mut |e| {
                // There should be no generic types in identities.
                if let Err(e) = self.update_generic_args_for_expression(e, &Default::default()) {
                    // TODO cannot borrow the expression here for printing it.
                    // We should fix this properly by using source references.
                    errors.push(format!(
                        "Error specializing generic references in expression:\n{e}",
                    ))
                }
            });
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.into_iter().join("\n"))
        }
    }

    /// Updates the type annotations in the literals and the generic arguments.
    fn update_generic_args_for_expression<T: FieldElement>(
        &self,
        e: &mut Expression<T>,
        type_var_mapping: &HashMap<String, Type>,
    ) -> Result<(), String> {
        match e {
            Expression::Number(n, annotated_type) => match annotated_type {
                Some(TypeName::Int) | Some(TypeName::Fe) | Some(TypeName::Expr) => {}
                Some(TypeName::TypeVar(tv)) => {
                    let mut ty = Type::TypeVar(tv.clone());
                    // Apply regular substitution obtained from unification.
                    self.substitute(&mut ty);
                    if !ty
                        .contained_type_vars()
                        .all(|tv| type_var_mapping.contains_key(tv))
                    {
                        Err(format!("Unable to derive concrete type for literal {n}."))?;
                    }
                    // Rename type vars (hopefully just a single one) to match the declaration scheme.
                    ty.substitute_type_vars(type_var_mapping);
                    if let Type::TypeVar(tv) = ty {
                        *annotated_type = Some(TypeName::TypeVar(tv.clone()));
                    } else {
                        match ty {
                            Type::Int => *annotated_type = Some(TypeName::Int),
                            Type::Fe => *annotated_type = Some(TypeName::Fe),
                            Type::Expr => *annotated_type = Some(TypeName::Expr),
                            t => panic!("Invalid resolved type literal number: {t}"),
                        }
                    }
                }
                _ => panic!("Invalid annotation for literal number."),
            },
            Expression::Reference(Reference::Poly(PolynomialReference {
                name,
                poly_id: _,
                generic_args,
            })) => {
                for ty in generic_args {
                    // Apply regular substitution obtained from unification.
                    self.substitute(ty);
                    // Now rename remaining type vars to match the declaration scheme.
                    // The remaining type vars need to be in the declaration scheme.
                    if !ty
                        .contained_type_vars()
                        .all(|tv| type_var_mapping.contains_key(tv))
                    {
                        Err(format!(
                            "Unable to derive concrete type for reference to generic symbol {name}"
                        ))?;
                    }
                    ty.substitute_type_vars(type_var_mapping);
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Type-checks the isolated expressions.
    fn check_expressions<T: FieldElement>(
        &mut self,
        expressions: &mut [(&mut Expression<T>, ExpectedType)],
    ) -> Result<(), String> {
        for (e, expected_type) in expressions {
            if expected_type.allow_array {
                self.process_expression(e)
                    .and_then(|ty| {
                        let ty = self.substitute_to(ty);
                        let expected_type = if matches!(ty, Type::Array(_)) {
                            Type::Array(ArrayType {
                                base: Box::new(expected_type.ty.clone()),
                                length: None,
                            })
                        } else {
                            expected_type.ty.clone()
                        };

                        self.unifier
                            .unify_types(ty.clone(), expected_type.clone())
                            .map_err(|err| {
                                format!(
                                    "Expected type {} but got type {}.\n{err}",
                                    self.substitute_to(expected_type),
                                    self.substitute_to(ty)
                                )
                            })
                    })
                    .map_err(|err| {
                        format!(
                            "Expression is expected to evaluate to {} or ({})[]:\n  {e}:\n{err}",
                            expected_type.ty, expected_type.ty
                        )
                    })?;
            } else {
                self.expect_type(&expected_type.ty, e)?;
            }
        }
        Ok(())
    }

    fn process_expression<T: FieldElement>(
        &mut self,
        e: &mut Expression<T>,
    ) -> Result<Type, String> {
        Ok(match e {
            Expression::Reference(Reference::LocalVar(id, _name)) => self.local_var_type(*id),
            Expression::Reference(Reference::Poly(PolynomialReference {
                name,
                poly_id: _,
                generic_args,
            })) => {
                // The generic args (some of them) could be pre-filled by the parser, but we do not yet support that.
                assert!(generic_args.is_empty());
                let (ty, gen_args) = self.instantiate_scheme(self.declared_types[name].clone());
                *generic_args = gen_args;
                type_for_reference(&ty)
            }
            Expression::PublicReference(_) => Type::Expr,
            Expression::Number(_, annotated_type) => {
                let ty = match annotated_type {
                    Some(TypeName::Int) => Type::Int,
                    Some(TypeName::Fe) => Type::Fe,
                    Some(TypeName::Expr) => Type::Expr,
                    Some(t) => panic!("Type name annotation for number is not supported: {t}"),
                    None => {
                        let tv = self.new_type_var_name();
                        *annotated_type = Some(TypeName::TypeVar(tv.clone()));
                        Type::TypeVar(tv)
                    }
                };
                self.unifier.ensure_bound(&ty, "FromLiteral".to_string())?;
                ty
            }
            Expression::String(_) => Type::String,
            Expression::Tuple(items) => Type::Tuple(TupleType {
                items: items
                    .iter_mut()
                    .map(|item| self.process_expression(item))
                    .collect::<Result<_, _>>()?,
            }),
            Expression::LambdaExpression(LambdaExpression { params, body }) => {
                let param_types = (0..params.len())
                    .map(|_| self.new_type_var())
                    .collect::<Vec<_>>();
                self.push_new_local_vars(param_types);
                let body_type_result = self.process_expression(body);
                let param_types = self.pop_local_var_types(params.len());
                let body_type = body_type_result?;
                Type::Function(FunctionType {
                    params: param_types,
                    value: Box::new(body_type),
                })
            }
            Expression::ArrayLiteral(ArrayLiteral { items }) => {
                let item_type = self.new_type_var();
                for e in items {
                    self.expect_type(&item_type, e)?;
                }

                Type::Array(ArrayType {
                    base: Box::new(item_type.clone()),
                    length: None,
                })
            }
            Expression::BinaryOperation(left, op, right) => {
                // TODO at some point, also store the generic args for operators
                let fun_type = self.instantiate_scheme(binary_operator_scheme(*op)).0;
                self.process_function_call(
                    fun_type,
                    [left, right].into_iter().map(AsMut::as_mut),
                    || format!("applying operator {op}"),
                )?
            }
            Expression::UnaryOperation(op, inner) => {
                // TODO at some point, also store the generic args for operators
                let fun_type = self.instantiate_scheme(unary_operator_scheme(*op)).0;
                self.process_function_call(
                    fun_type,
                    [inner].into_iter().map(AsMut::as_mut),
                    || format!("applying unary {op}"),
                )?
            }
            Expression::IndexAccess(IndexAccess { array, index }) => {
                let result = self.new_type_var();
                self.expect_type(
                    &Type::Array(ArrayType {
                        base: Box::new(result.clone()),
                        length: None,
                    }),
                    array,
                )?;

                self.expect_type(&Type::Int, index)?;
                result
            }
            Expression::FunctionCall(FunctionCall {
                function,
                arguments,
            }) => {
                let ft = self.process_expression(function)?;
                self.process_function_call(ft, arguments.iter_mut(), || {
                    format!("calling function {function}")
                })?
            }
            Expression::FreeInput(_) => todo!(),
            Expression::MatchExpression(scrutinee, arms) => {
                let scrutinee_type = self.process_expression(scrutinee)?;
                let result = self.new_type_var();
                for MatchArm { pattern, value } in arms {
                    if let MatchPattern::Pattern(pattern) = pattern {
                        self.expect_type(&scrutinee_type, pattern)?;
                    }
                    self.expect_type(&result, value)?;
                }
                result
            }
            Expression::IfExpression(if_expr) => {
                self.expect_type(&Type::Bool, &mut if_expr.condition)?;
                let result = self.process_expression(&mut if_expr.body)?;
                self.expect_type(&result, &mut if_expr.else_body)?;
                result
            }
        })
    }

    fn process_function_call<'b, T: FieldElement>(
        &mut self,
        function_type: Type,
        arguments: impl ExactSizeIterator<Item = &'b mut Expression<T>>,
        error_message: impl FnOnce() -> String,
    ) -> Result<Type, String> {
        let arguments = arguments.collect::<Vec<_>>();
        let params = (0..arguments.len())
            .map(|_| self.new_type_var())
            .collect::<Vec<_>>();
        let result_type = self.new_type_var();
        let expected_function_type = Type::Function(FunctionType {
            params: params.clone(),
            value: Box::new(result_type.clone()),
        });
        self.unifier
            .unify_types(function_type.clone(), expected_function_type.clone())
            .map_err(|err| {
                // TODO the error message is a bit weird here. In the future, this
                // should just use source locations.
                format!(
                    "Expected function of type `{}`, but got `{}` when {} on ({}):\n{err}",
                    self.substitute_to(expected_function_type),
                    self.substitute_to(function_type),
                    error_message(),
                    arguments.iter().format(", ")
                )
            })?;

        for (arg, param) in arguments.into_iter().zip(params) {
            self.expect_type(&param, arg)?;
        }
        Ok(result_type)
    }

    fn expect_type<T: FieldElement>(
        &mut self,
        expected_type: &Type,
        expr: &mut Expression<T>,
    ) -> Result<(), String> {
        let inferred_type = self.process_expression(expr)?;
        self.unifier
            .unify_types(inferred_type.clone(), expected_type.clone())
            .map_err(|err| {
                format!(
                    "Error checking sub-expression {expr}:\nExpected type: {}\nInferred type: {}\n{err}",
                    self.substitute_to(expected_type.clone()),
                    self.substitute_to(inferred_type)
                )
            })
    }

    fn substitute_to(&self, mut ty: Type) -> Type {
        self.substitute(&mut ty);
        ty
    }

    fn substitute(&self, ty: &mut Type) {
        ty.substitute_type_vars(self.unifier.substitutions());
    }

    /// Instantiates a type scheme by creating new type variables for the quantified
    /// type variables in the scheme and adds the required trait bounds for the
    /// new type variables.
    /// Returns the new type and a vector of the type variables used for those
    /// declared in the scheme.
    fn instantiate_scheme(&mut self, scheme: TypeScheme) -> (Type, Vec<Type>) {
        let mut ty = scheme.ty;
        let vars = scheme
            .vars
            .bounds()
            .map(|(_, bounds)| {
                let new_var = self.new_type_var();
                for b in bounds {
                    self.unifier.ensure_bound(&new_var, b.clone()).unwrap();
                }
                new_var
            })
            .collect::<Vec<_>>();
        let substitutions = scheme.vars.vars().cloned().zip(vars.clone()).collect();
        ty.substitute_type_vars(&substitutions);
        (ty, vars)
    }

    fn new_type_var_name(&mut self) -> String {
        self.last_type_var += 1;
        format!("T{}", self.last_type_var)
    }

    fn new_type_var(&mut self) -> Type {
        Type::TypeVar(self.new_type_var_name())
    }

    /// Creates a type scheme out of a type by making all unsubstituted
    /// type variables generic.
    /// TODO this is wrong for mutually recursive generic functions.
    fn to_type_scheme(&self, ty: Type) -> TypeScheme {
        let ty = self.substitute_to(ty);
        let vars = TypeBounds::new(ty.contained_type_vars().map(|v| {
            (
                v.clone(),
                self.unifier
                    .type_var_bounds(v)
                    .into_iter()
                    .collect::<BTreeSet<_>>(),
            )
        }));
        TypeScheme { vars, ty }.simplify_type_vars()
    }

    pub fn local_var_type(&self, id: u64) -> Type {
        self.local_var_types[id as usize].clone()
    }

    pub fn push_new_local_vars(&mut self, types: Vec<Type>) {
        self.local_var_types = [types, self.local_var_types.clone()].concat();
    }

    pub fn pop_local_var_types(&mut self, count: usize) -> Vec<Type> {
        self.local_var_types.drain(0..count).collect()
    }
}
