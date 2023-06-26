use super::matcher::Matcher;
use crate::{ActError, ActResult, Candidate, Context, Subject};

pub fn parse(ctx: &Context, sub: &Subject) -> ActResult<(Matcher, Candidate)> {
    let matcher = Matcher::parse(&sub.matcher)?;
    let cands = parse_cands(ctx, &sub.cands)?;
    Ok((matcher, cands))
}

fn parse_cands(ctx: &Context, expr: &str) -> ActResult<Candidate> {
    let mut cands = Vec::new();
    if expr.is_empty() {
        return Err(ActError::Runtime("subject's cands is required".to_string()));
    }

    let ret = ctx.eval_with::<rhai::Array>(expr);
    match ret {
        Ok(users) => {
            let users: Vec<_> = users
                .iter()
                .map(|c| Candidate::parse(&c.clone().to_string()).unwrap())
                .collect();

            cands.extend(users);
        }
        Err(err) => return Err(err),
    }

    Ok(Candidate::Set(cands))
}
