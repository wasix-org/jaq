use crate::error::{Error, Type};
use crate::results::then;
use crate::val::{Val, ValR, ValRs};
use alloc::{boxed::Box, rc::Rc, vec::Vec};
pub use jaq_syn::path::Opt;

#[derive(Clone, Debug)]
pub struct Path<F>(pub Vec<(Part<F>, Opt)>);

#[derive(Clone, Debug)]
pub enum Part<I> {
    Index(I),
    /// if both are `None`, return iterator over whole array/object
    Range(Option<I>, Option<I>),
}

impl Path<Vec<Val>> {
    pub fn collect(&self, v: Val) -> Result<Vec<Val>, Error> {
        self.0.iter().try_fold(Vec::from([v]), |acc, (part, opt)| {
            opt.collect(acc.into_iter().flat_map(|x| part.collect(x)))
        })
    }

    pub fn update<'f>(&self, v: Val, f: impl Fn(Val) -> ValRs<'f> + Copy) -> ValRs<'f> {
        Part::update(self.0.iter(), v, f)
    }
}

impl Part<Vec<Val>> {
    pub fn collect(&self, current: Val) -> ValRs {
        use core::iter::once;
        match self {
            Self::Index(indices) => match current {
                Val::Arr(a) => Box::new(indices.iter().map(move |i| {
                    Ok(abs_index(i.as_int()?, a.len())
                        .map(|i| a[i].clone())
                        .unwrap_or(Val::Null))
                })),
                Val::Obj(o) => Box::new(indices.iter().map(move |i| match i {
                    Val::Str(s) => Ok(o.get(&**s).cloned().unwrap_or(Val::Null)),
                    i => Err(Error::Index(Val::Obj(o.clone()), i.clone())),
                })),
                _ => Box::new(once(Err(Error::Type(current, Type::Iter)))),
            },
            Self::Range(None, None) => then(current.try_into_iter(), |iter| Box::new(iter.map(Ok))),
            Self::Range(from, until) => match current {
                Val::Arr(a) => {
                    let len = a.len();
                    let from = rel_bounds(from).map(move |i| Ok(abs_bound(i?, len, 0)));
                    let until = rel_bounds(until).map(move |i| Ok(abs_bound(i?, len, len)));
                    Box::new(prod(from, until).map(move |(from, until)| {
                        let (skip, take) = skip_take(from?, until?);
                        Ok(Val::arr(a.iter().skip(skip).take(take).cloned().collect()))
                    }))
                }
                Val::Str(s) => {
                    let len = s.chars().count();
                    let from = rel_bounds(from).map(move |i| Ok(abs_bound(i?, len, 0)));
                    let until = rel_bounds(until).map(move |i| Ok(abs_bound(i?, len, len)));
                    Box::new(prod(from, until).map(move |(from, until)| {
                        let (skip, take) = skip_take(from?, until?);
                        Ok(Val::str(s.chars().skip(skip).take(take).collect()))
                    }))
                }
                _ => Box::new(once(Err(Error::Type(current, Type::Range)))),
            },
        }
    }

    pub fn update<'a, 'f, P, F>(mut path: P, v: Val, f: F) -> ValRs<'f>
    where
        P: Iterator<Item = &'a (Self, Opt)> + Clone,
        F: Fn(Val) -> ValRs<'f> + Copy,
    {
        if let Some((p, opt)) = path.next() {
            let f = |v| Self::update(path.clone(), v, f);
            Box::new(core::iter::once(p.map(v, *opt, f)))
        } else {
            f(v)
        }
    }

    pub fn map<F, I>(&self, mut v: Val, opt: Opt, f: F) -> ValR
    where
        F: Fn(Val) -> I,
        I: Iterator<Item = ValR>,
    {
        use Opt::{Essential, Optional};
        match self {
            Self::Index(indices) => match v {
                Val::Obj(ref mut o) => {
                    let o = Rc::make_mut(o);
                    for i in indices.iter() {
                        use indexmap::map::Entry::{Occupied, Vacant};
                        match (i, opt) {
                            (Val::Str(s), _) => match o.entry(Rc::clone(s)) {
                                Occupied(mut e) => {
                                    match f(e.get().clone()).next().transpose()? {
                                        Some(y) => e.insert(y),
                                        None => e.remove(),
                                    };
                                }
                                Vacant(e) => {
                                    if let Some(y) = f(Val::Null).next().transpose()? {
                                        e.insert(y);
                                    }
                                }
                            },
                            (i, Essential) => return Err(Error::Index(v, i.clone())),
                            (_, Optional) => (),
                        }
                    }
                    Ok(v)
                }
                Val::Arr(ref mut a) => {
                    let a = Rc::make_mut(a);
                    for i in indices.iter() {
                        let abs_or = |i| abs_index(i, a.len()).ok_or(Error::IndexOutOfBounds(i));
                        let i = match (i.as_int().and_then(abs_or), opt) {
                            (Ok(i), _) => i,
                            (Err(e), Essential) => return Err(e),
                            (Err(_), Optional) => continue,
                        };

                        if let Some(y) = f(a[i].clone()).next().transpose()? {
                            a[i] = y;
                        } else {
                            a.remove(i);
                        }
                    }
                    Ok(v)
                }
                _ => opt.fail(v, |v| Error::Type(v, Type::Iter)),
            },
            Self::Range(None, None) => match v.try_map(&f)? {
                y @ (Val::Arr(_) | Val::Obj(_)) => Ok(y),
                v => opt.fail(v, |v| Error::Type(v, Type::Iter)),
            },
            Self::Range(from, until) => match v {
                Val::Arr(ref mut a) => {
                    let a = Rc::make_mut(a);
                    for (from, until) in prod(rel_bounds(from), rel_bounds(until)) {
                        let (from, until) = match (from.and_then(|from| Ok((from, until?))), opt) {
                            (Ok(from_until), _) => from_until,
                            (Err(e), Essential) => return Err(e),
                            (Err(_), Optional) => continue,
                        };

                        let len = a.len();
                        let from = abs_bound(from, len, 0);
                        let until = abs_bound(until, len, len);

                        let (skip, take) = skip_take(from, until);
                        let arr = Val::arr(a.iter().skip(skip).take(take).cloned().collect());
                        let y = f(arr).map(|y| y?.into_arr()).next().transpose()?;
                        a.splice(skip..skip + take, (*y.unwrap_or_default()).clone());
                    }
                    Ok(v)
                }
                _ => opt.fail(v, |v| Error::Type(v, Type::Arr)),
            },
        }
    }
}

impl<F> Path<F> {
    pub fn eval<'a>(&'a self, run: impl Fn(&'a F) -> ValRs<'a>) -> Result<Path<Vec<Val>>, Error> {
        let path = self.0.iter().map(|(p, opt)| Ok((p.eval(&run)?, *opt)));
        Ok(Path(path.collect::<Result<_, _>>()?))
    }
}

impl<F> Part<F> {
    fn eval<'a>(&'a self, run: impl Fn(&'a F) -> ValRs<'a>) -> Result<Part<Vec<Val>>, Error> {
        use Part::{Index, Range};
        match self {
            Index(i) => Ok(Index(run(i).collect::<Result<_, _>>()?)),
            Range(from, until) => {
                let from = from.as_ref().map(|f| run(f).collect());
                let until = until.as_ref().map(|u| run(u).collect());
                Ok(Range(from.transpose()?, until.transpose()?))
            }
        }
    }
}

impl<F> From<Part<F>> for Path<F> {
    fn from(p: Part<F>) -> Self {
        Self(Vec::from([(p, Opt::Essential)]))
    }
}

type RelBounds<'a> = Box<dyn Iterator<Item = Result<Option<isize>, Error>> + 'a>;
fn rel_bounds(f: &Option<Vec<Val>>) -> RelBounds<'_> {
    match f {
        Some(f) => Box::new(f.iter().map(move |i| Ok(Some(i.as_int()?)))),
        None => Box::new(core::iter::once(Ok(None))),
    }
}

fn prod<'a, T: 'a + Clone>(
    l: impl Iterator<Item = T> + 'a,
    r: impl Iterator<Item = T> + 'a,
) -> impl Iterator<Item = (T, T)> + 'a {
    let r: Vec<_> = r.collect();
    l.flat_map(move |l| r.clone().into_iter().map(move |r| (l.clone(), r)))
}

fn skip_take(from: usize, until: usize) -> (usize, usize) {
    (from, if until > from { until - from } else { 0 })
}

/// If a range bound is given, absolutise and clip it between 0 and `len`,
/// else return `default`.
fn abs_bound(i: Option<isize>, len: usize, default: usize) -> usize {
    let abs = |i| core::cmp::min(wrap(i, len).unwrap_or(0), len);
    i.map(abs).unwrap_or(default)
}

/// Absolutise an index and return result if it is inside [0, len).
fn abs_index(i: isize, len: usize) -> Option<usize> {
    wrap(i, len).filter(|i| *i < len)
}

fn wrap(i: isize, len: usize) -> Option<usize> {
    if i >= 0 {
        Some(i as usize)
    } else if len < -i as usize {
        None
    } else {
        Some(len - (-i as usize))
    }
}

#[test]
fn wrap_test() {
    let len = 4;
    assert_eq!(wrap(0, len), Some(0));
    assert_eq!(wrap(8, len), Some(8));
    assert_eq!(wrap(-1, len), Some(3));
    assert_eq!(wrap(-4, len), Some(0));
    assert_eq!(wrap(-8, len), None);
}
