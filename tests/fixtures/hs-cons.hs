module Lists where

import Prelude hiding (Functor(..), Applicative(..), Monad(..))

data List a = Nil | Cons a (List a)
   deriving (Show)

class Functor f where
   fmap :: (a -> b) -> f a -> f b

class Functor f => Applicative f where
   pure  :: a -> f a
   (<*>) :: f (a -> b) -> f a -> f b

class Applicative m => Monad m where
   (>>=) :: m a -> (a -> m b) -> m b

instance Functor List where
   fmap _ Nil = Nil
   fmap f (Cons x xs) = Cons (f x) (fmap f xs)

instance Applicative List where
   pure x = Cons x Nil
   Nil <*> _ = Nil
   (Cons f fs) <*> xs = append (fmap f xs) (fs <*> xs)
   where
      append Nil ys = ys
      append (Cons a as) ys = Cons a (append as ys)

instance Monad List where
   Nil >>= _ = Nil
   <|>
