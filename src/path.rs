use crate::filter::Filter;
use crate::val::{Val, Vals};
use std::convert::TryInto;
use std::rc::Rc;

pub type Path = Vec<PathElem>;

#[derive(Debug)]
pub enum PathElem {
    Index(Filter),
    /// if both are `None`, return iterator over whole array/object
    Range(Option<Filter>, Option<Filter>),
}

fn get_index(i: &Val, len: usize) -> usize {
    match *i {
        Val::Num(i) => {
            let i = i.to_isize().unwrap();
            if i < 0 {
                (len as isize + i).try_into().unwrap_or(0)
            } else {
                i as usize
            }
        }
        _ => panic!("cannot index array with non-numeric index"),
    }
}

impl PathElem {
    pub fn follow(&self, root: Rc<Val>, current: Val) -> Vals {
        match self {
            Self::Index(filter) => {
                let index = filter.run(root);
                match current {
                    Val::Arr(vals) => {
                        Box::new(index.map(move |i| Rc::clone(&vals[get_index(&i, vals.len())])))
                    }
                    Val::Obj(o) => Box::new(index.map(move |i| match &*i {
                        Val::Str(s) => Rc::clone(&o.get(s).unwrap()),
                        _ => todo!(),
                    })),
                    _ => panic!("index"),
                }
            }
            Self::Range(None, None) => match current {
                Val::Arr(a) => Box::new(a.into_iter()),
                Val::Obj(o) => Box::new(o.into_iter().map(|(_k, v)| v)),
                _ => todo!(),
            },
            Self::Range(from, until) => match current {
                Val::Arr(a) => {
                    use core::iter::once;
                    let len = a.len();
                    let from = match from {
                        Some(from) => {
                            Box::new(from.run(Rc::clone(&root)).map(move |i| get_index(&i, len)))
                        }
                        None => Box::new(once(0 as usize)) as Box<dyn Iterator<Item = _>>,
                    };
                    let until = match until {
                        Some(until) => Box::new(until.run(root).map(|i| get_index(&i, len))),
                        None => Box::new(once(a.len())) as Box<dyn Iterator<Item = _>>,
                    };
                    let until: Vec<_> = until.collect();
                    use itertools::Itertools;
                    let from_until = from.into_iter().cartesian_product(until);
                    Box::new(from_until.map(move |(from, until)| {
                        let take = if until > from { until - from } else { 0 };
                        Rc::new(Val::Arr(a.iter().cloned().skip(from).take(take).collect()))
                    }))
                }
                _ => todo!(),
            },
        }
    }
}

/*
enum OnError {
    Empty,
    Fail,
}
*/
