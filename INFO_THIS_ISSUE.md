# This file
contains notes for how to complete this issue.  
(https://github.com/mythril-hypervisor/mythril/issues/64).

## Dan Slack Messages
gist of issue: `mythril_core` has its own error type  
`core/src/error.rs:69`  
good coding style thing to do in rust

so every (just about) file `use crate::error::{Result, Error}` and function
bubbling up an error will then be `fn(...) -> Result<T>`  
`core/src/error.rs:86`  

handy TryFromPrimitive crate that implements `#[derive(TryFromPrimitive)]`
for enums `core/src/device/pit.rs:11` so don't have to hand roll it
`core/src/ioapic:262`.

Problem is that it doesn't return a usable error type so in the function
`fn (...) -> Result<T>` can't run `Foo::try_from(x)?`  
`num_enum` does have a usable error type that is returned so that if we  
implement `From<num_enum::TryFromPrimitiveError<...>>` we can use  
`Foo::try_from(x)?` in a function with the signature `fn (...) -> Result<T>`  


## From the issue page
switch to num\_enum (crate) for `TryFromPrimitive` implementation  
so that we can make dealing with `try_from` errors more ergonomic.

#### Details
derive-try-from-primitive crate does not make the returned error type public.  
This makes dealing with an error from `try_from` cumbersome to deal with.  
The num\_enum crate does make the returned error type of `try_from` public.  
We should implement the following in `mythril_core/src/error.rs`:
```
impl<T: TryFromPrimitive> From<TryFromPrimitiveError<T>> for Error {
    //  ...
}
```  
and then switch all uses of `derive_try_from_primitive` to `num_enum`.



## Trait stuff
Conditionally implement methods on a generic type depending on trait bounds.
```
impl<T: Display + PartialOrd> Pair<T> {
    fn cmp_display(&self) {
    }
}
```
Using a trait bound with an impl block that uses generic type parameters,  
can implement methods conditionally for types that implement the specified  
traits.  

Example:    `struct Pair<T>` only implements the `cmp_display` method if its  
inner  type `T` implements `PartialOrd` and `Display`


Also can conditionally implement a trait for any type that implements another  
trait. Implementations on any type that satisfies the trait bounds are called  
*blanket implementations*.  
Exxample:   stdlib implemnts `ToString` trait for any type that implements the  
`Display` trait. looks like:  
```
impl<T: Display> ToString for T {
    //  ...
}
```

### Questions
should implement `From` trait when given type is the error type from `num_enum`  
and the returned type is our error type.  
Need this because the question operator will inject  
`if is error { return From::from(unwrapped_err); }`  
And the type param T is for the TTryfromPrimitiveError, which is generic over  
types implementing `TryFromPrimitive`


